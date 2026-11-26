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

function show(state: { local: LocalModels; endpoints?: StudioEndpoint[]; prefs?: StudioPrefs }) {
  useStudio.setState({ local: state.local, endpoints: state.endpoints ?? [], prefs: state.prefs ?? prefs(), loaded: true });
  render(<Notices tab="image" onError={vi.fn()} />);
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
