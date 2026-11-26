// Studio's empty states (0.1.0 Task B2; 0.2.0 Task U9, design 10.4): an icon, one line and one
// primary action. With no model on this computer, *Get models* opens Get models; on first use,
// the card names the model that fits this computer, as 0.1.2 did, and its button opens that
// model's detail panel.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { formatBytes } from "../../lib/format";
import { LOCAL, MODELS } from "./fixtures/models";
import type { LocalModels, StudioEndpoint, StudioPrefs } from "./types";

vi.mock("../../lib/ipc", () => ({ isTauri: () => true }));
vi.mock("./api");

const { Notices } = await import("./EmptyStates");
const { useStudio } = await import("../../store/studio");

const INITIAL = useStudio.getState();
/** The first recommended entry: FLUX.2 klein 4B, about 5.3 GB. */
const klein = LOCAL.recommended[0]!;
/** Nothing installed. */
const NONE: LocalModels = { ...LOCAL, installed: [] };
/** Nothing installed, and nothing in the catalog fits this computer. */
const NOTHING_FITS: LocalModels = { ...NONE, recommended: [], notHere: LOCAL.recommended, videoOnProcessor: true };
/** Picture models installed, no video model, and no video model in the catalog that fits: a
 *  computer that runs models on its processor (Task G2 lets it make video there, slowly). */
const PICTURES_ONLY: LocalModels = {
  ...LOCAL,
  installed: LOCAL.installed.filter((m) => m.kind === "image"),
  recommended: LOCAL.recommended.filter((e) => e.kind === "image"),
  notHere: [...LOCAL.notHere, ...LOCAL.recommended.filter((e) => e.kind === "video")],
  videoOnProcessor: true,
};
/** The refusal 0.1.x said for video on such a computer: never beside a Get models button. */
const REFUSAL = /needs a graphics card/;

function prefs(patch: Partial<StudioPrefs> = {}): StudioPrefs {
  return {
    lastModel: {},
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

function show(state: { local: LocalModels; endpoints?: StudioEndpoint[]; prefs?: StudioPrefs; tab?: "image" | "video" }) {
  const tab = state.tab ?? "image";
  useStudio.setState({ local: state.local, endpoints: state.endpoints ?? [], prefs: state.prefs ?? prefs(), loaded: true, tab });
  render(<Notices tab={tab} onError={vi.fn()} />);
}

async function click(el: HTMLElement) {
  await act(async () => {
    fireEvent.click(el);
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  useStudio.setState(INITIAL, true);
});

afterEach(cleanup);

describe("Studio's empty states", () => {
  it("with no local models it says so and Get models opens Get models", async () => {
    show({ local: NOTHING_FITS });
    const card = screen.getByText("No models on this computer yet").closest("section")!;
    // An icon, the line, and one primary action (the cloud backup is a quiet second line).
    expect(card.querySelector("svg[aria-hidden]")).not.toBeNull();
    const primary = within(card).getByRole("button", { name: "Get models" });
    expect(primary.className).toContain("btn-primary");
    expect(card.querySelectorAll(".btn-primary")).toHaveLength(1);
    within(card).getByText(
      "This computer cannot run local models well. Cloud models run on a company's computers instead: add OpenRouter or OpenAI.",
    );
    within(card).getByRole("button", { name: "Open Endpoints" });
    await click(primary);
    expect(useStudio.getState().localView).toBe(true);
    // The list, not a model's panel.
    expect(useStudio.getState().detail).toBeNull();
  });

  // [U9 fix round 1] The headline is true only when nothing at all is installed here, and its
  // second line never refuses what its button offers.
  it("nothing installed shows 'No models on this computer yet', on either tab, with a warning and no refusal", async () => {
    show({ local: NOTHING_FITS, tab: "video" });
    const card = screen.getByText("No models on this computer yet").closest("section")!;
    within(card).getByText(
      "This computer cannot run local video models well. OpenRouter makes videos in the cloud: add it in Settings → Endpoints.",
    );
    expect(card.textContent).not.toMatch(REFUSAL);
    await click(within(card).getByRole("button", { name: "Get models" }));
    expect(useStudio.getState().localView).toBe(true);
    expect(useStudio.getState().searchFilter).toBe("video");
  });

  it("with a picture model installed, the Video tab says no video models, never that nothing is on this computer", () => {
    show({ local: PICTURES_ONLY, tab: "video" });
    expect(screen.queryByText("No models on this computer yet")).toBeNull();
    const card = screen.getByText("No video models on this computer yet").closest("section")!;
    expect(card.querySelector("svg[aria-hidden]")).not.toBeNull();
    expect(card.querySelectorAll(".btn-primary")).toHaveLength(1);
    // Its second line warns; it never says this computer cannot make video at all.
    within(card).getByText(
      "This computer cannot run local video models well. OpenRouter makes videos in the cloud: add it in Settings → Endpoints.",
    );
    expect(card.textContent).not.toMatch(REFUSAL);
    within(card).getByRole("button", { name: "Open Endpoints" });
  });

  it("the video card's Get models opens Get models on its Video filter", async () => {
    // The store's tab is still Pictures: the card's own kind decides the filter.
    show({ local: PICTURES_ONLY, tab: "video" });
    useStudio.setState({ tab: "image", searchFilter: "pictures" });
    const card = screen.getByText("No video models on this computer yet").closest("section")!;
    await click(within(card).getByRole("button", { name: "Get models" }));
    expect(useStudio.getState().localView).toBe(true);
    expect(useStudio.getState().searchFilter).toBe("video");
    expect(useStudio.getState().detail).toBeNull();
  });

  it("with a video model installed that cannot run here yet, the Video tab keeps the cloud card", () => {
    // Wan 2.2 is installed, but this computer makes no video on its processor before Task G2.
    show({ local: { ...LOCAL, videoOnProcessor: true }, tab: "video" });
    expect(screen.queryByText(/^No (video )?models on this computer yet$/)).toBeNull();
    expect(screen.queryByRole("button", { name: "Get models" })).toBeNull();
    screen.getByText(
      "Video on this computer needs a graphics card or an Apple-silicon Mac. OpenRouter makes videos in the cloud: add it in Settings → Endpoints.",
    );
    screen.getByRole("button", { name: "Open Endpoints" });
  });

  it("on first use it names the model that fits, and its button opens that model's detail panel", async () => {
    show({ local: NONE });
    const card = screen.getByText(`${klein.name} fits this computer (${formatBytes(klein.sizeBytes)}).`).closest("section")!;
    within(card).getByText("Make pictures and videos on this computer: free, and nothing is sent.");
    expect(card.querySelector("svg[aria-hidden]")).not.toBeNull();
    const primary = within(card).getByRole("button", { name: `Get ${klein.name}` });
    expect(card.querySelectorAll(".btn-primary")).toHaveLength(1);
    await click(primary);
    expect(useStudio.getState().localView).toBe(true);
    expect(useStudio.getState().detail).toEqual({ entryId: klein.id });
  });

  it("the one-time suggestion opens the model it names in its detail panel", async () => {
    const openrouter: StudioEndpoint = {
      providerId: 1,
      name: "OpenRouter",
      company: "openrouter",
      host: "openrouter.ai",
      cloud: true,
      hasKey: true,
      fetchedAt: 1,
      models: MODELS,
    };
    // A cloud model to use, so the tab is not empty; nothing installed here yet.
    show({ local: NONE, endpoints: [openrouter], prefs: prefs({ suggestionDismissed: false }) });
    const card = screen.getByText(`Studio works on this computer. ${klein.name} fits here: free and private.`).closest("section")!;
    await click(within(card).getByRole("button", { name: `Get ${klein.name}` }));
    expect(useStudio.getState().detail).toEqual({ entryId: klein.id });
  });
});
