// Studio's tray of added pictures (0.1.0 Task B5; the composer's strip since Task U3): a picture
// dropped, pasted or chosen is read as the chat composer reads one and kept by the backend
// (`studio_add_ref`), then shows as a thumbnail with its Remove button, and a role chip when the
// model takes more than one role; the tray refuses a picture past the model's limit. Pictures
// of the other roles show in their own zones (InputZones.tsx).
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
const { RefTray, useNativePictureDrops, NATIVE_DROP_SETTLE_MS } = await import("./RefTray");
const { InputZone } = await import("./InputZones");
const { UiProviders } = await import("../ui/UiProviders");
const { useStudio } = await import("../../store/studio");

const INITIAL = useStudio.getState();

function model(id: string): MediaModel {
  const found = MODELS.find((m) => m.id === id);
  if (!found) throw new Error(`no fixture model ${id}`);
  return found;
}
const flare = model("openai/gpt-image-2.6-flare");
const veo = model("google/veo-3.1");
/** GPT Image 2.6 Flare taking a start frame too: a model with two roles. */
const flareFramed: MediaModel = {
  ...flare,
  params: [
    ...flare.params,
    { ...veo.params.find((p) => p.kind.type === "images" && p.kind.role === "startFrame")! },
  ],
};
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

/** The strip's *Add picture* button. */
function addButton(): HTMLElement {
  return screen.getByRole("button", { name: "Add picture" });
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
  it("an added picture goes to studioAddRef and shows as a thumbnail with its Remove button", async () => {
    await renderTray(flare);
    vi.mocked(api.studioAddRef).mockResolvedValue({ sha256: "a".repeat(64), mime: "image/png", sizeBytes: 1200, role: null });
    dropFile(addButton(), pngFile());
    await waitFor(() => expect(api.studioAddRef).toHaveBeenCalledTimes(1));
    expect(api.studioAddRef).toHaveBeenCalledWith({ mime: "image/png", fileName: "a.png", data: "iVBORw0KGgo=" });
    await screen.findByRole("button", { name: "Remove picture 1" });
    screen.getByRole("img", { name: "Picture 1" });
    // Flare takes only reference pictures: no role to choose.
    expect(screen.queryByRole("button", { name: "Reference" })).toBeNull();
    expect(refs()).toEqual([{ sha256: "a".repeat(64), mime: "image/png", sizeBytes: 1200, role: "reference" }]);
  });

  it("the tray refuses a picture past the model's limit", async () => {
    await renderTray(flareOne);
    vi.mocked(api.studioAddRef).mockResolvedValueOnce(kept(1)).mockResolvedValueOnce(kept(2));
    dropFile(addButton(), pngFile());
    await screen.findByRole("button", { name: "Remove picture 1" });
    dropFile(addButton(), pngFile("b.png"));
    await screen.findByText("This model takes 1 added picture.");
    // Refused before the picture was kept.
    expect(api.studioAddRef).toHaveBeenCalledTimes(1);
    expect(refs()).toHaveLength(1);
  });

  it("several pictures at once are added in order, up to the limit", async () => {
    await renderTray(veo);
    vi.mocked(api.studioAddRef).mockResolvedValueOnce(kept(1)).mockResolvedValueOnce(kept(2));
    // Veo takes no reference pictures (no Add picture), but a paste still finds its frames.
    expect(screen.queryByRole("button", { name: "Add picture" })).toBeNull();
    fireEvent.paste(screen.getByRole("textbox", { name: "Prompt" }), {
      clipboardData: { files: [pngFile("a.png"), pngFile("b.png"), pngFile("c.png")] },
    });
    await screen.findByText("This model takes 2 added pictures.");
    // A video's pictures go to its frames in the model's order: the start frame, then the end.
    expect(refs("video").map((r) => r.role)).toEqual(["startFrame", "endFrame"]);
    expect(api.studioAddRef).toHaveBeenCalledTimes(2);
  });

  it("the role chip moves a picture to another role the model takes, and out of the strip into its zone", async () => {
    await renderTray(flareFramed);
    vi.mocked(api.studioAddRef).mockResolvedValue(kept(1));
    dropFile(addButton(), pngFile());
    const chip = await screen.findByRole("button", { name: "Reference" });
    await act(async () => {
      fireEvent.pointerDown(chip, { button: 0, ctrlKey: false });
    });
    expect(screen.getAllByRole("menuitemradio").map((i) => i.textContent)).toEqual(["Reference", "Start frame"]);
    await act(async () => {
      fireEvent.click(screen.getByRole("menuitemradio", { name: "Start frame" }));
    });
    expect(refs().map((r) => r.role)).toEqual(["startFrame"]);
    expect(screen.queryByRole("button", { name: "Remove picture 1" })).toBeNull();
  });

  it("an added picture can be removed", async () => {
    await renderTray(flare);
    vi.mocked(api.studioAddRef).mockResolvedValue(kept(1));
    dropFile(addButton(), pngFile());
    fireEvent.click(await screen.findByRole("button", { name: "Remove picture 1" }));
    expect(refs()).toEqual([]);
    expect(screen.queryByRole("button", { name: "Remove picture 1" })).toBeNull();
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
    dropFile(addButton(), new File(["hello"], "notes.txt", { type: "text/plain" }));
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
    dropFile(addButton(), pngFile());
    await screen.findByText("The disk is full.");
    // With what to do about it.
    screen.getByText("Free up space.");
    expect(screen.getByRole("alert").textContent).toBe("The disk is full. Free up space.");
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
      dropFile(addButton(), pngFile());
      // Named as a picture from the start; its bytes, once read, are the picture.
      await screen.findByRole("img", { name: "Picture 1" });
      await waitFor(() => expect(screen.getByRole("img", { name: "Picture 1" }).getAttribute("src")).toBe("blob:thumb"));
      expect(api.studioRefBytes).toHaveBeenCalledWith(kept(1).sha256);
      fireEvent.click(screen.getByRole("button", { name: "Remove picture 1" }));
      expect(revokeObjectURL).toHaveBeenCalledWith("blob:thumb");
    } finally {
      URL.createObjectURL = original.create;
      URL.revokeObjectURL = original.revoke;
    }
  });

  it("a picture pasted past the limit says why where it can be seen", async () => {
    await renderTray(flareOne);
    vi.mocked(api.studioAddRef).mockResolvedValue(kept(1));
    fireEvent.paste(screen.getByRole("textbox", { name: "Prompt" }), { clipboardData: { files: [pngFile()] } });
    await screen.findByRole("button", { name: "Remove picture 1" });
    fireEvent.paste(screen.getByRole("textbox", { name: "Prompt" }), { clipboardData: { files: [pngFile("b.png")] } });
    expect((await screen.findByRole("alert")).textContent).toBe("This model takes 1 added picture.");
    expect(api.studioAddRef).toHaveBeenCalledTimes(1);
  });

  it("a picture refused because the model changed while it was kept says why", async () => {
    await renderTray(flare);
    let keep: (ref: RefItem) => void = () => undefined;
    vi.mocked(api.studioAddRef).mockImplementation(() => new Promise((resolve) => (keep = resolve)));
    fireEvent.paste(screen.getByRole("textbox", { name: "Prompt" }), { clipboardData: { files: [pngFile()] } });
    await waitFor(() => expect(api.studioAddRef).toHaveBeenCalledTimes(1));
    // Meanwhile the model became one that takes no pictures: the tray has nothing else to show.
    const none = { ...flare, params: flare.params.filter((p) => p.kind.type !== "images") };
    act(() => useStudio.setState({ endpoints: [endpoint([none, ...MODELS.filter((x) => x.id !== flare.id)])] }));
    expect(screen.queryByRole("button", { name: "Add picture" })).toBeNull();
    await act(async () => keep(kept(1)));
    expect((await screen.findByRole("alert")).textContent).toBe("This model takes no added pictures.");
    expect(refs()).toEqual([]);
  });

  it("a picture pasted while the model takes none is left to the box it was pasted in", async () => {
    await renderTray({ ...flare, params: flare.params.filter((p) => p.kind.type !== "images") });
    // Not taken over: the paste goes on as it would without Studio.
    const notPrevented = fireEvent.paste(screen.getByRole("textbox", { name: "Prompt" }), {
      clipboardData: { files: [pngFile()] },
    });
    expect(notPrevented).toBe(true);
    await flush();
    expect(api.studioAddRef).not.toHaveBeenCalled();
    expect(useStudio.getState().drafts.image.refs).toEqual([]);
  });

  it("removing a picture does not read the others' bytes again", async () => {
    const original = { create: URL.createObjectURL, revoke: URL.revokeObjectURL };
    URL.createObjectURL = vi.fn(() => "blob:thumb");
    URL.revokeObjectURL = vi.fn();
    try {
      await renderTray(flare);
      vi.mocked(api.studioRefBytes).mockResolvedValue(new Uint8Array([1, 2, 3]).buffer);
      const three = [1, 2, 3].map((n) => ({ ...kept(n), role: "reference" as const }));
      act(() => {
        useStudio.setState((s) => ({ drafts: { ...s.drafts, image: { ...s.drafts.image, refs: three } } }));
      });
      await waitFor(() => expect(api.studioRefBytes).toHaveBeenCalledTimes(3));
      await flush();
      vi.mocked(api.studioRefBytes).mockClear();
      fireEvent.click(screen.getByRole("button", { name: "Remove picture 1" }));
      await flush();
      expect(refs().map((r) => r.sha256)).toEqual([kept(2).sha256, kept(3).sha256]);
      expect(screen.getAllByRole("img").map((i) => i.getAttribute("alt"))).toEqual(["Picture 1", "Picture 2"]);
      expect(api.studioRefBytes).not.toHaveBeenCalled();
    } finally {
      URL.createObjectURL = original.create;
      URL.revokeObjectURL = original.revoke;
    }
  });

  it("the same picture twice keeps each one's thumbnail when the first goes", async () => {
    const original = { create: URL.createObjectURL, revoke: URL.revokeObjectURL };
    URL.createObjectURL = vi.fn(() => "blob:thumb");
    URL.revokeObjectURL = vi.fn();
    try {
      await renderTray(flare);
      vi.mocked(api.studioRefBytes).mockResolvedValue(new Uint8Array([1, 2, 3]).buffer);
      const twice = [
        { ...kept(1), role: "reference" as const },
        { ...kept(1), role: "reference" as const },
      ];
      act(() => {
        useStudio.setState((s) => ({ drafts: { ...s.drafts, image: { ...s.drafts.image, refs: twice } } }));
      });
      await waitFor(() => expect(api.studioRefBytes).toHaveBeenCalledTimes(2));
      await flush();
      fireEvent.click(screen.getByRole("button", { name: "Remove picture 1" }));
      await flush();
      expect(refs()).toHaveLength(1);
      screen.getByRole("img", { name: "Picture 1" });
      expect(api.studioRefBytes).toHaveBeenCalledTimes(2);
    } finally {
      URL.createObjectURL = original.create;
      URL.revokeObjectURL = original.revoke;
    }
  });

  it("a model that takes no pictures has no tray", async () => {
    await renderTray({ ...flare, params: flare.params.filter((p) => p.kind.type !== "images") });
    expect(screen.queryByRole("button", { name: "Add picture" })).toBeNull();
    expect(useStudio.getState().addRef("image", kept(1))).toBe("This model takes no added pictures.");
  });
});

// On Windows the window keeps Tauri's drag-and-drop, and the webview's own drop carries no
// files: a picture dropped from the file manager comes as Tauri's native drop, which goes to
// the tray or input zone under it; the backend keeps the files from the paths it recorded
// (`studio_add_dropped`, by their place in the drop). On macOS and Linux both drops come, and
// the webview's own takes them.
describe("a native drop (Windows)", () => {
  type OnDrop = (count: number, at: { x: number; y: number }) => void;
  let dropped: OnDrop = () => undefined;

  function Following(props: { tab: "image" | "video" }) {
    useNativePictureDrops(true);
    const spec = useStudio((s) => {
      const { target, model } = s.drafts[props.tab];
      const m = target && model ? s.endpoints[0]?.models.find((x) => x.id === model) : undefined;
      return m?.params.find((p) => p.kind.type === "images" && p.kind.role === "startFrame") ?? null;
    });
    return (
      <UiProviders>
        <div data-studio-view>
          <section data-composer>
            <RefTray tab={props.tab} />
            {spec && <InputZone tab={props.tab} spec={spec} />}
            <textarea aria-label="Prompt" />
          </section>
        </div>
      </UiProviders>
    );
  }

  async function follow(m: MediaModel) {
    vi.mocked(api.onNativeDrop).mockImplementation(async (handler: OnDrop) => {
      dropped = handler;
      return () => undefined;
    });
    await renderTray(m);
    cleanup();
    await act(async () => {
      render(<Following tab={m.kind} />);
    });
    await flush();
  }

  /** A native drop of `count` files on `el` (found under the drop's point). */
  async function nativeDrop(el: Element, count = 1) {
    document.elementFromPoint = vi.fn(() => el);
    act(() => dropped(count, { x: 10, y: 20 }));
    await act(async () => {
      vi.advanceTimersByTime(NATIVE_DROP_SETTLE_MS);
    });
    await flush();
  }

  beforeEach(() => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "Date"] });
    // Well after the webview's own drops of the tests before.
    vi.setSystemTime(Date.now() + 60_000);
  });
  afterEach(() => {
    vi.useRealTimers();
    delete (document as { elementFromPoint?: unknown }).elementFromPoint;
  });

  it("files dropped on the prompt are added from the drop, by their place in it", async () => {
    await follow(flare);
    vi.mocked(api.studioAddDropped).mockResolvedValueOnce(kept(1)).mockResolvedValueOnce(kept(2));
    await nativeDrop(screen.getByRole("textbox", { name: "Prompt" }), 2);
    expect(api.studioAddDropped).toHaveBeenNthCalledWith(1, 0, null);
    expect(api.studioAddDropped).toHaveBeenNthCalledWith(2, 1, null);
    expect(refs().map((r) => r.sha256)).toEqual([kept(1).sha256, kept(2).sha256]);
    expect(api.studioAddRef).not.toHaveBeenCalled();
  });

  it("a file dropped on the start frame zone becomes the start frame", async () => {
    await follow(veo);
    vi.mocked(api.studioAddDropped).mockResolvedValue(kept(3));
    await nativeDrop(screen.getByRole("button", { name: "Add the start frame" }), 2);
    expect(api.studioAddDropped).toHaveBeenCalledTimes(1);
    expect(refs("video")).toEqual([{ ...kept(3), role: "startFrame" }]);
  });

  it("past the model's limit it is refused before anything is kept, and a refusal says why", async () => {
    await follow(flareOne);
    vi.mocked(api.studioAddDropped).mockResolvedValueOnce(kept(1));
    await nativeDrop(addButton(), 2);
    screen.getByText("This model takes 1 added picture.");
    expect(api.studioAddDropped).toHaveBeenCalledTimes(1);
  });

  it("a drop the webview's own drop took (macOS, Linux) is not added twice", async () => {
    await follow(flare);
    vi.mocked(api.studioAddRef).mockResolvedValue(kept(1));
    const tray = addButton();
    dropFile(tray, pngFile());
    await nativeDrop(tray, 1);
    // The webview's own drop reads its file meanwhile.
    for (let i = 0; i < 5 && vi.mocked(api.studioAddRef).mock.calls.length === 0; i++) {
      await act(async () => {
        vi.advanceTimersByTime(100);
      });
      await flush();
    }
    expect(api.studioAddDropped).not.toHaveBeenCalled();
    expect(api.studioAddRef).toHaveBeenCalledTimes(1);
  });

  it("a drop elsewhere adds nothing", async () => {
    await follow(flare);
    await nativeDrop(document.body, 1);
    expect(api.studioAddDropped).not.toHaveBeenCalled();
  });
});
