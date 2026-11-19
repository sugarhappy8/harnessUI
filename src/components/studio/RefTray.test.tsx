// Studio's tray of added pictures (0.1.0 Task B5): a picture dropped, pasted or chosen is read
// as the composer reads one and kept by the backend (`studio_add_ref`), then shows with a role
// chip from the model's own picture fields; the tray refuses a picture past the model's limit.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { LOCAL, MODELS } from "./fixtures/models";
import type { MediaModel, RefItem, StudioEndpoint, StudioPrefs } from "./types";

// Radix positions menus with ResizeObserver, which jsdom does not have.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

vi.mock("../../lib/ipc", () => ({ isTauri: () => true }));
vi.mock("./api");

const api = await import("./api");
const { RefTray } = await import("./RefTray");
const { useStudio } = await import("../../store/studio");

const INITIAL = useStudio.getState();

function model(id: string): MediaModel {
  const found = MODELS.find((m) => m.id === id);
  if (!found) throw new Error(`no fixture model ${id}`);
  return found;
}
const flare = model("openai/gpt-image-2.6-flare");
const veo = model("google/veo-3.1");
/** GPT Image 2.6 Flare taking at most one reference picture. */
const flareOne: MediaModel = {
  ...flare,
  params: flare.params.map((p) =>
    p.kind.type === "images" ? { ...p, kind: { ...p.kind, max: 1 } } : p,
  ),
};

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

function endpoint(models: MediaModel[]): StudioEndpoint {
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

async function flush() {
  await act(async () => {
    for (let i = 0; i < 10; i++) await Promise.resolve();
  });
}

/** Studio open on `m` at OpenRouter, with the tray drawn inside the view (as the prompt dock
 *  draws it, beside the prompt box). */
async function renderTray(m: MediaModel) {
  vi.mocked(api.studioModels).mockResolvedValue({ endpoints: [endpoint([m, ...MODELS.filter((x) => x.id !== m.id)])] });
  vi.mocked(api.studioLocalModels).mockResolvedValue(LOCAL);
  vi.mocked(api.studioRuns).mockResolvedValue({ runs: [], next: null });
  vi.mocked(api.studioLive).mockResolvedValue([]);
  vi.mocked(api.studioGetPrefs).mockResolvedValue(prefs({ lastModel: { [m.kind]: { target: { endpoint: 1 }, model: m.id } } }));
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
  useStudio.getState().showStudio(m.kind);
  await act(async () => {
    render(
      <div data-studio-view>
        <RefTray tab={m.kind} />
        <textarea aria-label="Prompt" />
      </div>,
    );
  });
}

function pngFile(name = "a.png"): File {
  return new File([new Uint8Array([137, 80, 78, 71, 13, 10, 26, 10])], name, { type: "image/png" });
}

function dropFile(el: HTMLElement, ...files: File[]) {
  fireEvent.drop(el, { dataTransfer: { files, types: ["Files"] } });
}

/** The backend's answer for an added picture: kept, with no role yet. */
function kept(n: number): RefItem {
  return { sha256: String(n).repeat(64).slice(0, 64), mime: "image/png", sizeBytes: 1200, role: null };
}

function refs(tab: "image" | "video" = "image"): RefItem[] {
  return useStudio.getState().drafts[tab].refs;
}

beforeEach(() => {
  vi.clearAllMocks();
  useStudio.setState(INITIAL, true);
});

afterEach(() => {
  cleanup();
});

describe("RefTray", () => {
  it("an added picture goes to studioAddRef and shows a role chip", async () => {
    await renderTray(flare);
    vi.mocked(api.studioAddRef).mockResolvedValue({ sha256: "a".repeat(64), mime: "image/png", sizeBytes: 1200, role: null });
    dropFile(screen.getByText("+ Add picture"), pngFile());
    await waitFor(() => expect(api.studioAddRef).toHaveBeenCalledTimes(1));
    expect(api.studioAddRef).toHaveBeenCalledWith({ mime: "image/png", fileName: "a.png", data: "iVBORw0KGgo=" });
    await screen.findByRole("button", { name: "Reference" }); // the role chip
    expect(refs()).toEqual([{ sha256: "a".repeat(64), mime: "image/png", sizeBytes: 1200, role: "reference" }]);
  });

  it("the tray refuses a picture past the model's limit", async () => {
    await renderTray(flareOne);
    vi.mocked(api.studioAddRef).mockResolvedValueOnce(kept(1)).mockResolvedValueOnce(kept(2));
    dropFile(screen.getByText("+ Add picture"), pngFile());
    await screen.findByRole("button", { name: "Reference" });
    dropFile(screen.getByText("+ Add picture"), pngFile("b.png"));
    await screen.findByText("This model takes 1 added picture.");
    // Refused before the picture was kept.
    expect(api.studioAddRef).toHaveBeenCalledTimes(1);
    expect(refs()).toHaveLength(1);
  });

  it("several pictures at once are added in order, up to the limit", async () => {
    await renderTray(veo);
    vi.mocked(api.studioAddRef).mockResolvedValueOnce(kept(1)).mockResolvedValueOnce(kept(2));
    dropFile(screen.getByText("+ Add picture"), pngFile("a.png"), pngFile("b.png"), pngFile("c.png"));
    await screen.findByText("This model takes 2 added pictures.");
    // A video's pictures go to its frames in the model's order: the start frame, then the end.
    expect(refs("video").map((r) => r.role)).toEqual(["startFrame", "endFrame"]);
    expect(api.studioAddRef).toHaveBeenCalledTimes(2);
  });

  it("the role chip moves a picture to another role the model takes", async () => {
    await renderTray(veo);
    vi.mocked(api.studioAddRef).mockResolvedValue(kept(1));
    dropFile(screen.getByText("+ Add picture"), pngFile());
    const chip = await screen.findByRole("button", { name: "Start frame" });
    await act(async () => {
      fireEvent.pointerDown(chip, { button: 0, ctrlKey: false });
    });
    expect(screen.getAllByRole("menuitemradio").map((i) => i.textContent)).toEqual(["Start frame", "End frame"]);
    await act(async () => {
      fireEvent.click(screen.getByRole("menuitemradio", { name: "End frame" }));
    });
    expect(refs("video").map((r) => r.role)).toEqual(["endFrame"]);
    screen.getByRole("button", { name: "End frame" });
  });

  it("an added picture can be removed", async () => {
    await renderTray(flare);
    vi.mocked(api.studioAddRef).mockResolvedValue(kept(1));
    dropFile(screen.getByText("+ Add picture"), pngFile());
    await screen.findByRole("button", { name: "Reference" });
    fireEvent.click(screen.getByRole("button", { name: "Remove picture 1" }));
    expect(refs()).toEqual([]);
    expect(screen.queryByRole("button", { name: "Reference" })).toBeNull();
  });

  it("a picture pasted in Studio is added; one pasted elsewhere is not", async () => {
    await renderTray(flare);
    vi.mocked(api.studioAddRef).mockResolvedValue(kept(1));
    const outside = document.createElement("textarea");
    document.body.append(outside);
    try {
      fireEvent.paste(outside, { clipboardData: { files: [pngFile()] } });
      await flush();
      expect(api.studioAddRef).not.toHaveBeenCalled();
      fireEvent.paste(screen.getByRole("textbox", { name: "Prompt" }), { clipboardData: { files: [pngFile()] } });
      await waitFor(() => expect(api.studioAddRef).toHaveBeenCalledTimes(1));
    } finally {
      outside.remove();
    }
  });

  it("a picture chosen from a file is added", async () => {
    await renderTray(flare);
    vi.mocked(api.studioAddRef).mockResolvedValue(kept(1));
    fireEvent.change(screen.getByLabelText("Choose a picture"), { target: { files: [pngFile()] } });
    await waitFor(() => expect(refs()).toHaveLength(1));
  });

  it("a file that is not a picture says so and is not sent", async () => {
    await renderTray(flare);
    dropFile(screen.getByText("+ Add picture"), new File(["hello"], "notes.txt", { type: "text/plain" }));
    await screen.findByText("notes.txt is not a PNG, JPEG, GIF or WebP image.");
    expect(api.studioAddRef).not.toHaveBeenCalled();
  });

  it("a picture the backend could not keep says why", async () => {
    await renderTray(flare);
    vi.mocked(api.studioAddRef).mockRejectedValue({
      code: "studio_disk_full",
      message: "The disk is full.",
      nextStep: "Free up space.",
      details: "",
    });
    dropFile(screen.getByText("+ Add picture"), pngFile());
    await screen.findByText("The disk is full.");
    expect(refs()).toEqual([]);
  });

  it("shows each picture from its bytes", async () => {
    // jsdom has no blob: URLs.
    const original = { create: URL.createObjectURL, revoke: URL.revokeObjectURL };
    const createObjectURL = vi.fn(() => "blob:thumb");
    const revokeObjectURL = vi.fn();
    URL.createObjectURL = createObjectURL;
    URL.revokeObjectURL = revokeObjectURL;
    try {
      await renderTray(flare);
      vi.mocked(api.studioAddRef).mockResolvedValue(kept(1));
      vi.mocked(api.studioRefBytes).mockResolvedValue(new Uint8Array([1, 2, 3]).buffer);
      dropFile(screen.getByText("+ Add picture"), pngFile());
      const img = await screen.findByRole("img", { name: "Picture 1" });
      await waitFor(() => expect(img.getAttribute("src")).toBe("blob:thumb"));
      expect(api.studioRefBytes).toHaveBeenCalledWith(kept(1).sha256);
      fireEvent.click(screen.getByRole("button", { name: "Remove picture 1" }));
      expect(revokeObjectURL).toHaveBeenCalledWith("blob:thumb");
    } finally {
      URL.createObjectURL = original.create;
      URL.revokeObjectURL = original.revoke;
    }
  });

  it("a model that takes no pictures has no tray", async () => {
    await renderTray({ ...flare, params: flare.params.filter((p) => p.kind.type !== "images") });
    expect(screen.queryByText("+ Add picture")).toBeNull();
    expect(useStudio.getState().addRef("image", kept(1))).toBe("This model takes no added pictures.");
  });
});
