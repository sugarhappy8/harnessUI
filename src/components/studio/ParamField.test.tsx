// Studio's controls (0.1.0 Task B5; styled Radix controls since Task U8, critique §2.8): each kind
// of field drawn with its widget from the model's own settings, "Automatic" in the control until
// a field is set (critique §4.f: never a second label beside it), fields shown only while what
// they require is met, and values that no longer apply dropped from the draft.
import type { ReactElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render as rtlRender, screen, waitFor, type RenderResult } from "@testing-library/react";
import { dropUnmet, visibleFields, type Values } from "./canonical";
import { MODELS } from "./fixtures/models";
import type { Choice, Layer, MediaModel, ParamKind, ParamSpec, StudioEndpoint, StudioPrefs } from "./types";

// jsdom has none of what Radix selects and sliders measure and capture with.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;
Element.prototype.hasPointerCapture ??= () => false;
Element.prototype.setPointerCapture ??= () => {};
Element.prototype.releasePointerCapture ??= () => {};
Element.prototype.scrollIntoView ??= () => {};

vi.mock("../../lib/ipc", () => ({ isTauri: () => true }));
vi.mock("./api");

const api = await import("./api");
const { ParamField, widgetFor } = await import("./ParamField");
const { UiProviders } = await import("../ui/UiProviders");
const { useStudio } = await import("../../store/studio");

/** Renders inside the one tooltip provider the help buttons need, as the app does. */
function render(ui: ReactElement): RenderResult {
  return rtlRender(ui, { wrapper: UiProviders });
}

/** Opens a Radix select from its trigger. */
function press(trigger: HTMLElement) {
  fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false, pointerType: "mouse" });
}

/** Picks `option` in the open select. */
function pick(option: string) {
  fireEvent.click(screen.getByRole("option", { name: option }));
}

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

  it("draws Shape and How many by their kind, as any field (the panel leaves them to the composer)", () => {
    expect(widgetFor(spec({ canonical: "shape", kind: { type: "enum", values: choices(11) } }))).toBe("select");
    expect(widgetFor(spec({ canonical: "shape", kind: { type: "enum", values: choices(3) } }))).toBe("segmented");
    expect(widgetFor(spec({ canonical: "count", kind: { type: "range", min: 1, max: 10, step: 1 } }))).toBe("slider");
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

  it("reads Automatic (company default) until set, and Automatic for a model on this computer, as its chosen chip", () => {
    field(background, undefined);
    const automatic = screen.getByRole("radio", { name: "Automatic (company default)" });
    expect(automatic.getAttribute("aria-checked")).toBe("true");
    // Said once, in the control: never as a second label beside it (critique §4.f).
    expect(screen.getAllByText(/Automatic/)).toHaveLength(1);
    expect(screen.queryByRole("button", { name: "Reset Background" })).toBeNull();
    cleanup();
    field(background, undefined, { local: true });
    expect(screen.getByRole("radio", { name: "Automatic" }).getAttribute("aria-checked")).toBe("true");
  });

  it("a set field offers its reset, and Automatic is no longer the chosen chip", () => {
    const { onReset } = field(background, "transparent");
    expect(screen.getByRole("radio", { name: "Transparent" }).getAttribute("aria-checked")).toBe("true");
    expect(screen.getByRole("radio", { name: "Automatic (company default)" }).getAttribute("aria-checked")).toBe("false");
    fireEvent.click(screen.getByRole("button", { name: "Reset Background" }));
    expect(onReset).toHaveBeenCalledTimes(1);
    // Automatic, chosen again, resets it too.
    fireEvent.click(screen.getByRole("radio", { name: "Automatic (company default)" }));
    expect(onReset).toHaveBeenCalledTimes(2);
  });

  it("a short list is a set of radios; choosing one sends its value", () => {
    const { onChange } = field(background, undefined);
    expect(screen.getAllByRole("radio").map((r) => r.textContent)).toEqual([
      "Automatic (company default)",
      "Auto",
      "Transparent",
      "Opaque",
    ]);
    fireEvent.click(screen.getByRole("radio", { name: "Opaque" }));
    expect(onChange).toHaveBeenCalledWith("opaque");
  });

  it("a short list with a default the model names shows it chosen while unset, with no Automatic chip", () => {
    const memory = spec({
      key: "memory_saving",
      label: "Memory saving",
      layer: "advanced",
      kind: {
        type: "enum",
        values: [
          { value: "automatic", label: "Automatic" },
          { value: "off", label: "Off" },
          { value: "on", label: "On" },
        ],
      },
      companyDefault: "automatic",
    });
    field(memory, undefined, { local: true });
    expect(screen.getAllByRole("radio").map((r) => r.textContent)).toEqual(["Automatic", "Off", "On"]);
    expect(screen.getByRole("radio", { name: "Automatic" }).getAttribute("aria-checked")).toBe("true");
    // Not twice, as 0.1.2 had it (critique §4.f).
    expect(screen.getAllByText("Automatic")).toHaveLength(1);
  });

  it("a long list is a select that starts at Automatic, and Automatic resets it", () => {
    const { onChange } = field(quality, undefined);
    const select = screen.getByRole("combobox", { name: "Quality" });
    expect(select.textContent).toBe("Automatic (company default)");
    press(select);
    // Unset, Automatic is the placeholder, not a choice.
    expect(screen.getAllByRole("option").map((o) => o.textContent)).toEqual(["Auto", "Low", "Medium", "High", "Extra high", "Max"]);
    pick("Extra high");
    expect(onChange).toHaveBeenCalledWith("xhigh");
    cleanup();
    const { onReset } = field(quality, "high");
    expect(screen.getByRole("combobox", { name: "Quality" }).textContent).toBe("High");
    press(screen.getByRole("combobox", { name: "Quality" }));
    pick("Automatic (company default)");
    expect(onReset).toHaveBeenCalledTimes(1);
  });

  it("a range is a slider on the field's steps, within its range, its value said beside its name", () => {
    const { onChange } = field(compression, undefined);
    const slider = screen.getByRole("slider", { name: "Compression" });
    // Unset with no default named: it reads Automatic, resting mid-range (Task U9), and moves on
    // from there.
    screen.getByText("Automatic (company default)");
    expect(slider.getAttribute("aria-valuetext")).toBe("Automatic (company default)");
    fireEvent.keyDown(slider, { key: "ArrowRight" });
    expect(onChange).toHaveBeenLastCalledWith(51);
    fireEvent.keyDown(slider, { key: "End" });
    expect(onChange).toHaveBeenLastCalledWith(100);
    cleanup();
    field(compression, 80);
    screen.getByText("80%");
    expect(screen.getByRole("slider", { name: "Compression" }).getAttribute("aria-valuenow")).toBe("80");
    cleanup();
    // Unset with a default the model names: that value.
    const cfg = spec({ key: "cfg_scale", label: "Guidance", kind: { type: "range", min: 1, max: 20, step: 0.5 }, companyDefault: 5 });
    field(cfg, undefined, { local: true });
    screen.getByText("5");
    expect(screen.getByRole("slider", { name: "Guidance" }).getAttribute("aria-valuenow")).toBe("5");
    expect(screen.queryByText("Automatic")).toBeNull();
  });

  // [0.2.0 U9, from U8's review] An unset slider whose thumb sat at the minimum read as the
  // minimum chosen (screen f, Picture guidance).
  it("an unset slider's thumb is not at its minimum: it rests greyed at the default, else mid-range, until moved", () => {
    const guidance = spec({
      key: "img_cfg_scale",
      label: "Picture guidance",
      layer: "advanced",
      kind: { type: "range", min: 1, max: 20, step: 0.5 },
    });
    const { onChange } = field(guidance, undefined, { local: true });
    let thumb = screen.getByRole("slider", { name: "Picture guidance" });
    expect(thumb.getAttribute("aria-valuenow")).not.toBe("1");
    expect(thumb.getAttribute("aria-valuenow")).toBe("10.5");
    expect(thumb.getAttribute("aria-valuetext")).toBe("Automatic");
    // Greyed, with no filled track behind it: nothing is chosen.
    expect(thumb.hasAttribute("data-unset")).toBe(true);
    expect(document.querySelector("[data-slider-range]")!.hasAttribute("data-unset")).toBe(true);
    fireEvent.keyDown(thumb, { key: "ArrowLeft" });
    expect(onChange).toHaveBeenLastCalledWith(10);
    cleanup();
    // With a default the model names (here the recipe's), it rests there, greyed too.
    const steps = spec({ key: "steps", label: "Steps", kind: { type: "range", min: 1, max: 50, step: 1 }, companyDefault: 4 });
    field(steps, undefined, { local: true });
    thumb = screen.getByRole("slider", { name: "Steps" });
    expect(thumb.getAttribute("aria-valuenow")).toBe("4");
    expect(thumb.hasAttribute("data-unset")).toBe(true);
    cleanup();
    // Once set, it is the accent's, at its value.
    field(guidance, 1, { local: true });
    thumb = screen.getByRole("slider", { name: "Picture guidance" });
    expect(thumb.getAttribute("aria-valuenow")).toBe("1");
    expect(thumb.hasAttribute("data-unset")).toBe(false);
    expect(document.querySelector("[data-slider-range]")!.hasAttribute("data-unset")).toBe(false);
  });

  it("a switch shows the company's default until set", () => {
    const sound = spec({ key: "generate_audio", label: "Sound", kind: { type: "boolean" }, companyDefault: true });
    const { onChange } = field(sound, undefined);
    const toggle = screen.getByRole("switch", { name: "Sound" });
    expect(toggle.getAttribute("aria-checked")).toBe("true");
    expect(screen.queryByText(/Automatic/)).toBeNull();
    fireEvent.click(toggle);
    expect(onChange).toHaveBeenCalledWith(false);
  });

  it("a field's help is a tooltip on its help button, and describes the field", async () => {
    const help = "Picks the noise the picture starts from.";
    const seed = spec({ key: "seed", canonical: "seed", label: "Seed", layer: "advanced", help, kind: { type: "seed", max: 2147483647 } });
    field(seed, undefined);
    const group = screen.getByRole("group", { name: "Seed" });
    expect(document.getElementById(group.getAttribute("aria-describedby")!)?.textContent).toBe(help);
    // No native title tooltip (critique §2.8): a styled one, on hover or keyboard focus.
    expect(document.querySelector("[title]")).toBeNull();
    await act(async () => {
      screen.getByRole("button", { name: "About Seed" }).focus();
    });
    await waitFor(() => expect(screen.getByRole("tooltip").textContent).toContain(help));
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

  it("a size typed that does not fit is told, so Create waits while the boxes show it", () => {
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
    const onInvalid = vi.fn();
    const { unmount } = render(
      <ParamField spec={size} value="1024x512" onChange={vi.fn()} onReset={vi.fn()} onInvalid={onInvalid} />,
    );
    expect(onInvalid).toHaveBeenLastCalledWith(null);
    fireEvent.change(screen.getByRole("spinbutton", { name: "Width" }), { target: { value: "1000" } });
    // The draft keeps 1024 × 512 while the boxes show 1000 × 512.
    expect(onInvalid).toHaveBeenLastCalledWith("Both sides must be multiples of 16.");
    fireEvent.change(screen.getByRole("spinbutton", { name: "Width" }), { target: { value: "1008" } });
    expect(onInvalid).toHaveBeenLastCalledWith(null);
    fireEvent.change(screen.getByRole("spinbutton", { name: "Width" }), { target: { value: "1" } });
    expect(onInvalid).not.toHaveBeenLastCalledWith(null);
    // Gone, it holds nothing back.
    unmount();
    expect(onInvalid).toHaveBeenLastCalledWith(null);
  });

  describe("a text field", () => {
    const negative = spec({ key: "negative_prompt", label: "Leave out", layer: "advanced", kind: { type: "text", maxChars: 5 } });

    it("keeps within its limit, counted by character, not by UTF-16 unit", () => {
      const onChange = vi.fn();
      const onReset = vi.fn();
      const { rerender } = render(<ParamField spec={negative} value={undefined} onChange={onChange} onReset={onReset} />);
      screen.getByText("5 left");
      // Six emoji are twelve UTF-16 units: five of them are kept, none cut in half.
      fireEvent.change(screen.getByRole("textbox", { name: "Leave out" }), { target: { value: "😀😀😀😀😀😀" } });
      expect(onChange).toHaveBeenLastCalledWith("😀😀😀😀😀");
      rerender(<ParamField spec={negative} value="a😀" onChange={onChange} onReset={onReset} />);
      expect((screen.getByRole("textbox", { name: "Leave out" }) as HTMLTextAreaElement).value).toBe("a😀");
      screen.getByText("3 left");
      expect(onReset).not.toHaveBeenCalled();
    });

    // 0.2.0 (design 9.2): the limit is a hidden bound, so its count shows only near it.
    it("shows what is left only when fewer than 1,000 characters remain", () => {
      const long = spec({ ...negative, kind: { type: "text", maxChars: 16_384 } });
      const left = () => screen.queryByText(/ left$/);
      const { rerender } = render(<ParamField spec={long} value={undefined} onChange={vi.fn()} onReset={vi.fn()} />);
      expect(left()).toBeNull();
      rerender(<ParamField spec={long} value={"a".repeat(15_384)} onChange={vi.fn()} onReset={vi.fn()} />);
      expect(left()).toBeNull();
      rerender(<ParamField spec={long} value={"a".repeat(15_385)} onChange={vi.fn()} onReset={vi.fn()} />);
      expect(left()?.textContent).toBe("999 left");
    });

    it("emptied, the field is unset", () => {
      const { onChange, onReset } = field(negative, "blur");
      fireEvent.change(screen.getByRole("textbox", { name: "Leave out" }), { target: { value: "" } });
      expect(onReset).toHaveBeenCalledTimes(1);
      expect(onChange).not.toHaveBeenCalled();
    });
  });

  describe("a size with listed sizes", () => {
    const rules = { multipleOf: 16, minRatio: 0.25, maxRatio: 4, maxSide: 2048, minPixels: 16384, maxPixels: 4194304 };
    const listedSize = spec({
      key: "size",
      canonical: "size",
      label: "Size",
      kind: {
        type: "size",
        rules,
        values: [
          { value: "1024x1024", label: "1024 × 1024" },
          { value: "1536x1024", label: "1536 × 1024" },
        ],
      },
    });
    const freeSize = spec({ key: "size", canonical: "size", label: "Size", kind: { type: "size", rules, values: [] } });
    const select = () => screen.getByRole("combobox", { name: "Size" });
    const choose = (option: string) => {
      press(select());
      pick(option);
    };
    const box = (name: "Width" | "Height") => screen.getByRole("spinbutton", { name }) as HTMLInputElement;

    it("is a select starting at Automatic; choosing a listed size sends it", () => {
      const { onChange } = field(listedSize, undefined);
      expect(select().textContent).toBe("Automatic (company default)");
      expect(screen.queryByRole("spinbutton", { name: "Width" })).toBeNull();
      choose("1536 × 1024");
      expect(onChange).toHaveBeenCalledWith("1536x1024");
    });

    it("shows the size set, Custom size with its sides when it is not in the list", () => {
      field(listedSize, "1536x1024");
      expect(select().textContent).toBe("1536 × 1024");
      expect(screen.queryByRole("spinbutton", { name: "Width" })).toBeNull();
      cleanup();
      field(listedSize, "1280x720");
      expect(select().textContent).toBe("Custom size");
      expect([box("Width").value, box("Height").value]).toEqual(["1280", "720"]);
    });

    it("a size not in the list, set from the store, shows the boxes with its numbers", () => {
      const sized: MediaModel = { ...flare, params: [listedSize] };
      renderFields(sized, "simple", {});
      expect(select().textContent).toBe("Automatic (company default)");
      // As a carry or a setup writes it: not chosen in the select.
      setValue("size", "1280x720");
      expect(select().textContent).toBe("Custom size");
      expect([box("Width").value, box("Height").value]).toEqual(["1280", "720"]);
      setValue("size", "1024x1024");
      expect(select().textContent).toBe("1024 × 1024");
      expect(screen.queryByRole("spinbutton", { name: "Width" })).toBeNull();
    });

    it("Custom size opens empty boxes until a size is typed, and Automatic closes them", () => {
      const { onChange } = field(listedSize, undefined);
      choose("Custom size");
      expect(select().textContent).toBe("Custom size");
      expect([box("Width").value, box("Height").value]).toEqual(["", ""]);
      expect(onChange).not.toHaveBeenCalled();
      fireEvent.change(box("Width"), { target: { value: "1280" } });
      fireEvent.change(box("Height"), { target: { value: "720" } });
      expect(onChange).toHaveBeenLastCalledWith("1280x720");
      cleanup();
      const again = field(listedSize, undefined);
      choose("Custom size");
      box("Width");
      // Nothing typed yet: Automatic, back in the list, puts the boxes away.
      choose("Automatic (company default)");
      expect(screen.queryByRole("spinbutton", { name: "Width" })).toBeNull();
      expect(select().textContent).toBe("Automatic (company default)");
      expect(again.onReset).toHaveBeenCalled();
    });

    it("Custom size over a listed size unsets it, and boxes emptied stay open", () => {
      const sized: MediaModel = { ...flare, params: [listedSize] };
      renderFields(sized, "simple", { size: "1024x1024" });
      choose("Custom size");
      // Nothing is sent until a custom size fits.
      expect(values()).toEqual({});
      expect(select().textContent).toBe("Custom size");
      expect([box("Width").value, box("Height").value]).toEqual(["", ""]);
      fireEvent.change(box("Width"), { target: { value: "1280" } });
      fireEvent.change(box("Height"), { target: { value: "720" } });
      expect(values()).toEqual({ size: "1280x720" });
      cleanup();
      // A custom size set from the store, emptied to type another: unset, and the boxes stay.
      renderFields(sized, "simple", { size: "1280x720" });
      fireEvent.change(box("Width"), { target: { value: "" } });
      fireEvent.change(box("Height"), { target: { value: "" } });
      expect(values()).toEqual({});
      expect(select().textContent).toBe("Custom size");
      box("Width");
    });

    it("a model whose size field lists nothing shows the boxes, under the same key too", () => {
      const onChange = vi.fn();
      const onReset = vi.fn();
      const { rerender } = render(<ParamField spec={listedSize} value={undefined} onChange={onChange} onReset={onReset} />);
      expect(screen.queryByRole("spinbutton", { name: "Width" })).toBeNull();
      // Another model, whose size field has the same key and lists no sizes.
      rerender(<ParamField spec={freeSize} value={undefined} onChange={onChange} onReset={onReset} />);
      expect(screen.queryByRole("combobox", { name: "Size" })).toBeNull();
      box("Width");
      box("Height");
    });

    it("a Custom size chosen is forgotten when the field changes to another model's", () => {
      const onChange = vi.fn();
      const onReset = vi.fn();
      const { rerender } = render(<ParamField spec={listedSize} value={undefined} onChange={onChange} onReset={onReset} />);
      choose("Custom size");
      box("Width");
      const other = spec({
        ...listedSize,
        kind: { type: "size", rules, values: [{ value: "768x768", label: "768 × 768" }] },
      });
      rerender(<ParamField spec={other} value={undefined} onChange={onChange} onReset={onReset} />);
      expect(select().textContent).toBe("Automatic (company default)");
      expect(screen.queryByRole("spinbutton", { name: "Width" })).toBeNull();
    });
  });

  it("asks nothing of the backend", () => {
    renderFields(flare, "simple", {});
    expect(Object.values(api).filter((f) => vi.isMockFunction(f) && f.mock.calls.length > 0)).toEqual([]);
  });
});
