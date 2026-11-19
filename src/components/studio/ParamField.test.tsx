// Studio's controls (0.1.0 Task B5): each kind of field drawn with its widget from the model's
// own settings, "Automatic" until a field is set, fields shown only while what they require is
// met, and values that no longer apply dropped from the draft.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { Values } from "./canonical";
import { MODELS } from "./fixtures/models";
import type { Choice, Layer, MediaModel, ParamKind, ParamSpec, StudioEndpoint, StudioPrefs } from "./types";

vi.mock("../../lib/ipc", () => ({ isTauri: () => true }));
vi.mock("./api");

const api = await import("./api");
const { ParamField, dropUnmet, visibleFields, widgetFor } = await import("./ParamField");
const { useStudio } = await import("../../store/studio");

const INITIAL = useStudio.getState();

function model(id: string): MediaModel {
  const found = MODELS.find((m) => m.id === id);
  if (!found) throw new Error(`no fixture model ${id}`);
  return found;
}
const flare = model("openai/gpt-image-2.6-flare");

/** A field with no role, in front, with nothing set by the company. */
function spec(patch: Partial<ParamSpec> & { kind: ParamKind }): ParamSpec {
  return {
    key: "field",
    canonical: null,
    label: "Field",
    help: null,
    layer: "simple",
    companyDefault: null,
    initial: null,
    affectsPrice: false,
    requires: null,
    ...patch,
  };
}

function choices(n: number): Choice[] {
  return Array.from({ length: n }, (_, i) => ({ value: `v${i}`, label: `V${i}` }));
}

function prefs(): StudioPrefs {
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
  };
}

/** OpenRouter with `models`. */
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

/** The Pictures tab's draft on `m` at OpenRouter, with `values`. */
function onModel(m: MediaModel, values: Values) {
  useStudio.setState((s) => ({
    endpoints: [endpoint([m, ...MODELS.filter((x) => x.id !== m.id)])],
    prefs: prefs(),
    drafts: { ...s.drafts, image: { ...s.drafts.image, target: { endpoint: 1 }, model: m.id, values } },
  }));
}

/** The fields of one layer as a panel draws them: through `visibleFields`, each changing the
 *  Pictures tab's draft through the store. */
function Fields(props: { model: MediaModel; layer: Layer }) {
  const values = useStudio((s) => s.drafts.image.values);
  const setValue = useStudio((s) => s.setValue);
  const resetValue = useStudio((s) => s.resetValue);
  return (
    <>
      {visibleFields(props.model.params, props.layer, values).map((f) => (
        <ParamField
          key={f.key}
          spec={f}
          value={values[f.key]}
          onChange={(v) => setValue("image", f.key, v)}
          onReset={() => resetValue("image", f.key)}
        />
      ))}
    </>
  );
}

function renderFields(m: MediaModel, layer: Layer, values: Values) {
  onModel(m, values);
  render(<Fields model={m} layer={layer} />);
}

function setValue(key: string, value: unknown) {
  act(() => useStudio.getState().setValue("image", key, value));
}

function values(): Values {
  return useStudio.getState().drafts.image.values;
}

beforeEach(() => {
  vi.clearAllMocks();
  useStudio.setState(INITIAL, true);
});

afterEach(() => {
  cleanup();
});

describe("widgetFor", () => {
  it("draws each kind with its widget", () => {
    expect(widgetFor(spec({ kind: { type: "enum", values: choices(5) } }))).toBe("segmented");
    expect(widgetFor(spec({ kind: { type: "enum", values: choices(6) } }))).toBe("select");
    expect(widgetFor(spec({ kind: { type: "range", min: 0, max: 100 } }))).toBe("slider");
    expect(widgetFor(spec({ kind: { type: "boolean" } }))).toBe("switch");
    expect(widgetFor(spec({ kind: { type: "seed", max: 2147483647 } }))).toBe("seed");
    expect(widgetFor(spec({ kind: { type: "images", min: 0, max: 1, role: "startFrame" } }))).toBe("drop");
    const rules = { multipleOf: 16, minRatio: 0.25, maxRatio: 4, maxSide: 2048, minPixels: 1, maxPixels: 9e6 };
    expect(widgetFor(spec({ kind: { type: "size", rules, values: [] } }))).toBe("size");
    expect(widgetFor(spec({ kind: { type: "text", maxChars: 500 } }))).toBe("text");
    expect(widgetFor(spec({ kind: { type: "extra", allowedKeys: ["moderation"] } }))).toBe("json");
  });

  it("draws Shape and How many as chips, whatever their size", () => {
    expect(widgetFor(spec({ canonical: "shape", kind: { type: "enum", values: choices(11) } }))).toBe("chips");
    expect(widgetFor(spec({ canonical: "shape", kind: { type: "enum", values: choices(3) } }))).toBe("chips");
    expect(widgetFor(spec({ canonical: "count", kind: { type: "range", min: 1, max: 10, step: 1 } }))).toBe("chips");
  });
});

describe("visibleFields", () => {
  it("keeps one layer, in the model's order", () => {
    expect(visibleFields(flare.params, "simple", {}).map((f) => f.key)).toEqual([
      "aspect_ratio",
      "n",
      "quality",
      "input_references",
    ]);
  });

  it("shows a field only while what it requires is met, an unset field counting as its company default", () => {
    const advanced = (v: Values) => visibleFields(flare.params, "advanced", v).map((f) => f.key);
    expect(advanced({})).toEqual(["output_format", "background", "extra"]);
    expect(advanced({ output_format: "webp" })).toEqual(["output_format", "output_compression", "background", "extra"]);
    expect(advanced({ output_format: "gif" })).toEqual([
      "output_format",
      "frame_count",
      "frame_duration_ms",
      "background",
      "extra",
    ]);
    const jpegByDefault = flare.params.map((f) => (f.key === "output_format" ? { ...f, companyDefault: "jpeg" } : f));
    expect(visibleFields(jpegByDefault, "advanced", {}).map((f) => f.key)).toContain("output_compression");
  });
});

describe("dropUnmet", () => {
  it("removes each value whose field no longer applies, and keeps the rest", () => {
    expect(dropUnmet(flare, { output_format: "png", output_compression: 80, quality: "high" })).toEqual({
      output_format: "png",
      quality: "high",
    });
    expect(dropUnmet(flare, { output_format: "jpeg", output_compression: 80 })).toEqual({
      output_format: "jpeg",
      output_compression: 80,
    });
  });

  it("an unset field counts as its company default", () => {
    expect(dropUnmet(flare, { frame_count: 8 })).toEqual({});
    const gifByDefault: MediaModel = {
      ...flare,
      params: flare.params.map((f) => (f.key === "output_format" ? { ...f, companyDefault: "gif" } : f)),
    };
    expect(dropUnmet(gifByDefault, { frame_count: 8 })).toEqual({ frame_count: 8 });
  });

  it("repeats until nothing more is removed", () => {
    // Detail needs Style "fine", which needs Mode "pro".
    const chained: MediaModel = {
      ...flare,
      params: [
        spec({ key: "mode", kind: { type: "enum", values: [{ value: "pro", label: "Pro" }, { value: "lite", label: "Lite" }] } }),
        spec({
          key: "detail",
          kind: { type: "range", min: 0, max: 10 },
          requires: { key: "style", values: ["fine"] },
        }),
        spec({
          key: "style",
          kind: { type: "enum", values: [{ value: "fine", label: "Fine" }] },
          requires: { key: "mode", values: ["pro"] },
        }),
      ],
    };
    expect(dropUnmet(chained, { mode: "lite", style: "fine", detail: 3 })).toEqual({ mode: "lite" });
    expect(dropUnmet(chained, { mode: "pro", style: "fine", detail: 3 })).toEqual({ mode: "pro", style: "fine", detail: 3 });
  });
});

describe("setting values", () => {
  it("hides compression unless the file type is JPEG or WebP", () => {
    renderFields(flare, "advanced", { output_format: "png" });
    expect(screen.queryByText("Compression")).toBeNull();
    setValue("output_format", "webp");
    screen.getByText("Compression");
  });

  it("choosing a transparent background switches to PNG and says so", () => {
    onModel(flare, { output_format: "jpeg" });
    setValue("background", "transparent");
    expect(values()).toEqual({ output_format: "png", background: "transparent" });
    expect(useStudio.getState().drafts.image.note).toBe(
      "Transparent needs PNG or WebP, so the file type is now PNG.",
    );
  });

  it("a transparent background keeps WebP, and an unset file type, as they are", () => {
    onModel(flare, { output_format: "webp" });
    setValue("background", "transparent");
    expect(values()).toEqual({ output_format: "webp", background: "transparent" });
    expect(useStudio.getState().drafts.image.note).toBeNull();
    onModel(flare, {});
    setValue("background", "transparent");
    expect(values()).toEqual({ background: "transparent" });
  });

  it("a file type that no longer takes compression drops it from the draft", () => {
    onModel(flare, { output_format: "webp", output_compression: 80 });
    setValue("output_format", "png");
    expect(values()).toEqual({ output_format: "png" });
  });

  it("switching to PNG for a transparent background drops compression too", () => {
    onModel(flare, { output_format: "jpeg", output_compression: 80 });
    setValue("background", "transparent");
    expect(values()).toEqual({ output_format: "png", background: "transparent" });
  });

  it("a reset field is unset, and what required it goes with it", () => {
    onModel(flare, { output_format: "webp", output_compression: 80, quality: "high" });
    act(() => useStudio.getState().resetValue("image", "output_format"));
    expect(values()).toEqual({ quality: "high" });
  });

  it("added pictures are never dropped with a value", () => {
    const ref = { sha256: "a".repeat(64), mime: "image/png", sizeBytes: 10, role: "reference" as const };
    onModel(flare, { output_format: "webp", output_compression: 80 });
    useStudio.setState((s) => ({ drafts: { ...s.drafts, image: { ...s.drafts.image, refs: [ref] } } }));
    setValue("output_format", "png");
    expect(useStudio.getState().drafts.image.refs).toEqual([ref]);
  });
});

describe("ParamField", () => {
  function field(f: ParamSpec, value: unknown, extra: { local?: boolean } = {}) {
    const onChange = vi.fn();
    const onReset = vi.fn();
    render(<ParamField spec={f} value={value} onChange={onChange} onReset={onReset} local={extra.local} />);
    return { onChange, onReset };
  }
  const background = flare.params.find((f) => f.key === "background")!;
  const quality = flare.params.find((f) => f.key === "quality")!;
  const compression = flare.params.find((f) => f.key === "output_compression")!;

  it("reads Automatic (company default) until set, and Automatic for a model on this computer", () => {
    field(background, undefined);
    screen.getByText("Automatic (company default)");
    expect(screen.queryByRole("button", { name: "Reset Background" })).toBeNull();
    cleanup();
    field(background, undefined, { local: true });
    screen.getByText("Automatic");
  });

  it("a set field offers its reset, and nothing reads Automatic", () => {
    const { onReset } = field(background, "transparent");
    expect(screen.getByRole("radio", { name: "Transparent" }).getAttribute("aria-checked")).toBe("true");
    expect(screen.queryByText("Automatic (company default)")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Reset Background" }));
    expect(onReset).toHaveBeenCalledTimes(1);
  });

  it("a short list is a set of radios; choosing one sends its value", () => {
    const { onChange } = field(background, undefined);
    expect(screen.getAllByRole("radio").map((r) => r.textContent)).toEqual(["Auto", "Transparent", "Opaque"]);
    fireEvent.click(screen.getByRole("radio", { name: "Opaque" }));
    expect(onChange).toHaveBeenCalledWith("opaque");
  });

  it("a long list is a select that starts at Automatic, and Automatic resets it", () => {
    const { onChange } = field(quality, undefined);
    const select = screen.getByRole("combobox", { name: "Quality" }) as HTMLSelectElement;
    expect(select.selectedOptions[0]?.textContent).toBe("Automatic (company default)");
    fireEvent.change(select, { target: { value: "4" } });
    expect(onChange).toHaveBeenCalledWith("xhigh");
    cleanup();
    const { onReset } = field(quality, "high");
    fireEvent.change(screen.getByRole("combobox", { name: "Quality" }), { target: { value: "" } });
    expect(onReset).toHaveBeenCalledTimes(1);
  });

  it("a number box takes a number when it is done, within the field's range", () => {
    const { onChange } = field(compression, undefined);
    const box = screen.getByRole("spinbutton", { name: "Compression" });
    fireEvent.change(box, { target: { value: "150" } });
    expect(onChange).not.toHaveBeenCalled();
    fireEvent.blur(box);
    expect(onChange).toHaveBeenCalledWith(100);
  });

  it("a switch shows the company's default until set", () => {
    const sound = spec({ key: "generate_audio", label: "Sound", kind: { type: "boolean" }, companyDefault: true });
    const { onChange } = field(sound, undefined);
    const toggle = screen.getByRole("switch", { name: "Sound" }) as HTMLInputElement;
    expect(toggle.checked).toBe(true);
    fireEvent.click(toggle);
    expect(onChange).toHaveBeenCalledWith(false);
  });

  it("an unset seed says Studio picks one", () => {
    const seed = spec({ key: "seed", canonical: "seed", label: "Seed", layer: "advanced", kind: { type: "seed", max: 2147483647 } });
    field(seed, undefined);
    expect(screen.getByRole("spinbutton", { name: "Seed" }).getAttribute("placeholder")).toBe("Random");
    expect(screen.queryByText("Automatic (company default)")).toBeNull();
  });

  it("an exact size is checked as it is typed, and sent only when it fits", () => {
    const size = spec({
      key: "size",
      canonical: "size",
      label: "Exact size",
      layer: "advanced",
      kind: {
        type: "size",
        rules: { multipleOf: 16, minRatio: 0.25, maxRatio: 4, maxSide: 2048, minPixels: 16384, maxPixels: 4194304 },
        values: [],
      },
    });
    const { onChange } = field(size, undefined);
    fireEvent.change(screen.getByRole("spinbutton", { name: "Width" }), { target: { value: "1000" } });
    fireEvent.change(screen.getByRole("spinbutton", { name: "Height" }), { target: { value: "512" } });
    screen.getByText("Both sides must be multiples of 16.");
    expect(onChange).not.toHaveBeenCalled();
    fireEvent.change(screen.getByRole("spinbutton", { name: "Width" }), { target: { value: "1024" } });
    expect(onChange).toHaveBeenLastCalledWith("1024x512");
    fireEvent.change(screen.getByRole("spinbutton", { name: "Height" }), { target: { value: "128" } });
    screen.getByText("The shape must be between 1:4 and 4:1.");
  });

  it("asks nothing of the backend", () => {
    renderFields(flare, "simple", {});
    expect(Object.values(api).filter((f) => vi.isMockFunction(f) && f.mock.calls.length > 0)).toEqual([]);
  });
});
