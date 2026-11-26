// Carrying settings from one model to another (0.1.0 Task B4): by what each field means, to
// the nearest value the new model takes, with one note that says what changed.
import { describe, expect, it } from "vitest";
import { SETTING_WORDS } from "../../lib/words";
import { carry, changedCount, changedKeys, closestModel, defaultsFor, familyOf, valueWords } from "./canonical";
import { LOCAL, MODELS } from "./fixtures/models";
import type { Lora, LoraUse, MediaModel, ParamKind, RefItem, RefRole, SdVersion } from "./types";

function byId(id: string): MediaModel {
  const model = MODELS.find((m) => m.id === id) ?? LOCAL.installed.find((i) => i.id === id)?.model;
  if (!model) throw new Error(`no fixture model ${id}`);
  return model;
}

const flare = byId("openai/gpt-image-2.6-flare");
const nanoBanana = byId("google/gemini-nano-banana-2.1");
const k3Edit = byId("krea/k3-edit");
const veo = byId("google/veo-3.1");
const grokLite = byId("x-ai/grok-imagine-video-1.5-lite");
const seedance = byId("bytedance/seedance-2.5-flash");
const klein = byId("flux2-klein-4b-q4");
const localVideo = byId("wan22-ti2v-5b-fast-q6");

/** A copy of `model` whose field `key` takes `kind` instead. */
function withKind(model: MediaModel, key: string, kind: ParamKind): MediaModel {
  return { ...model, params: model.params.map((p) => (p.key === key ? { ...p, kind } : p)) };
}

/** The choices of `model`'s list field `key` whose values are in `keep`. */
function choicesOf(model: MediaModel, key: string, keep: unknown[]): Extract<ParamKind, { type: "enum" }> {
  const kind = model.params.find((p) => p.key === key)?.kind;
  if (kind?.type !== "enum") throw new Error(`${key} is not a list`);
  return { type: "enum", values: kind.values.filter((c) => keep.includes(c.value)) };
}

function refs(n: number, role: RefRole | null = "reference"): RefItem[] {
  return Array.from({ length: n }, (_, i) => ({ sha256: `${role}-${i}`, mime: "image/png", sizeBytes: 1000, role }));
}

describe("carry", () => {
  it("names what changed, in one note", () => {
    const r = carry(k3Edit, nanoBanana, { aspect_ratio: "16:9", seed: 81723 }, {}, []); // K3 Edit takes a seed; Nano Banana does not
    expect(r.values).toEqual({ aspect_ratio: "16:9" });
    expect(r.note).toBe("Nano Banana 2.1 has no seed setting, so it was removed.");
  });

  it("says nothing when everything carries as it was", () => {
    const r = carry(flare, flare, { aspect_ratio: "auto", quality: "low", n: 2 }, { moderation: "low" }, refs(2));
    expect(r).toEqual({ values: { aspect_ratio: "auto", quality: "low", n: 2 }, extra: { moderation: "low" }, refs: refs(2), note: null });
  });

  it("moves a shape to the nearest ratio", () => {
    const r = carry(flare, k3Edit, { aspect_ratio: "21:9" }, {}, []); // K3 Edit has no 21:9
    expect(r.values.aspect_ratio).toBe("16:9");
    expect(r.note).toBe("Shape 21:9 became 16:9.");
  });

  it("keeps Auto only where it is offered", () => {
    expect(carry(flare, flare, { aspect_ratio: "auto" }, {}, []).values.aspect_ratio).toBe("auto");
    const r = carry(flare, k3Edit, { aspect_ratio: "auto" }, {}, []);
    expect(r.values).toEqual({});
    expect(r.note).toBe("K3 Edit does not offer shape Auto, so it was removed.");
  });

  it("lowers how many to the new maximum", () => {
    const removed = carry(flare, nanoBanana, { n: 4 }, {}, []);
    expect(removed.values).toEqual({});
    expect(removed.note).toBe("Nano Banana 2.1 has no how many setting, so it was removed.");
    const twoAtMost = withKind(flare, "n", { type: "range", min: 1, max: 2, step: 1, unit: null });
    const lowered = carry(flare, twoAtMost, { n: 4 }, {}, []);
    expect(lowered.values).toEqual({ n: 2 });
    expect(lowered.note).toBe("How many 4 became 2.");
  });

  it("moves a number into the field's range and onto its steps", () => {
    const r = carry(klein, klein, { steps: 60 }, {}, []);
    expect(r.values).toEqual({ steps: 50 });
    expect(r.note).toBe("Steps 60 became 50.");
    const halves = withKind(klein, "steps", { type: "range", min: 1, max: 20, step: 0.5, unit: null });
    expect(carry(klein, halves, { steps: 7.3 }, {}, []).values.steps).toBe(7.5);
  });

  it("removes a seed the new model cannot take", () => {
    const small = withKind(veo, "seed", { type: "seed", max: 1000 });
    const r = carry(k3Edit, small, { seed: 81723 }, {}, []);
    expect(r.values).toEqual(defaultsFor(veo));
    expect(r.note).toBe("Veo 3.1 does not offer seed 81723, so it was removed.");
  });

  it("keeps the quality name or moves one step cheaper", () => {
    const upToHigh = withKind(flare, "quality", choicesOf(flare, "quality", ["low", "medium", "high"]));
    const xhigh = carry(flare, upToHigh, { quality: "xhigh" }, {}, []);
    expect(xhigh.values.quality).toBe("high");
    expect(xhigh.note).toBe("Quality extra high became high."); // word values mid-sentence
    expect(carry(flare, upToHigh, { quality: "max" }, {}, []).values.quality).toBe("high");
    const low = carry(flare, upToHigh, { quality: "low" }, {}, []);
    expect(low.values.quality).toBe("low");
    expect(low.note).toBeNull();
  });

  it("moves quality to the cheapest offered when nothing cheaper is offered, and drops Auto where it is not", () => {
    const mediumUp = withKind(flare, "quality", choicesOf(flare, "quality", ["medium", "high"]));
    expect(carry(flare, mediumUp, { quality: "low" }, {}, []).values.quality).toBe("medium");
    const r = carry(flare, mediumUp, { quality: "auto" }, {}, []);
    expect(r.values).toEqual({});
    expect(r.note).toBe("GPT Image 2.6 Flare does not offer quality Auto, so it was removed.");
  });

  it("moves a length to the nearest one, the shorter on a tie", () => {
    const r = carry(grokLite, veo, { duration: 7 }, {}, []); // Veo: 4, 6, 8 s
    expect(r.values.duration).toBe(6);
    expect(r.note).toBe("Length 7 s became 6 s.");
    expect(carry(grokLite, veo, { duration: 15 }, {}, []).values.duration).toBe(8);
    const lengths = choicesOf(veo, "duration", [4, 6, 8]);
    const longestFirst = withKind(veo, "duration", { ...lengths, values: [...lengths.values].reverse() });
    expect(carry(grokLite, longestFirst, { duration: 7 }, {}, []).values.duration).toBe(6);
  });

  it("moves a resolution to the same or the next lower one", () => {
    const r = carry(veo, grokLite, { resolution: "4K" }, {}, []); // Grok: 480p, 720p, 1080p
    expect(r.values.resolution).toBe("1080p");
    expect(r.note).toBe("Resolution 4K became 1080p.");
    expect(carry(nanoBanana, k3Edit, { resolution: "4K" }, {}, []).values.resolution).toBe("2K");
    expect(carry(veo, grokLite, { resolution: "720p" }, {}, []).note).toBeNull();
  });

  it("moves a resolution below the lowest offered to the lowest", () => {
    const r = carry(grokLite, veo, { resolution: "480p" }, {}, []); // Veo: 720p, 1080p, 4K
    expect(r.values.resolution).toBe("720p");
    expect(r.note).toBe("Resolution 480p became 720p.");
  });

  it("keeps added pictures up to the new limit", () => {
    const threeAtMost = withKind(flare, "input_references", { type: "images", min: 0, max: 3, role: "reference" });
    const r = carry(flare, threeAtMost, {}, {}, refs(5));
    expect(r.refs).toEqual(refs(5).slice(0, 3));
    expect(r.note).toBe("Only 3 added pictures fit, so 2 were left out.");
    const oneAtMost = withKind(flare, "input_references", { type: "images", min: 0, max: 1, role: "reference" });
    expect(carry(flare, oneAtMost, {}, {}, refs(2)).note).toBe("Only 1 added picture fits, so 1 was left out.");
  });

  it("keeps each frame where the new model takes it", () => {
    const frames = [...refs(1, "startFrame"), ...refs(1, "endFrame")];
    const r = carry(veo, grokLite, {}, {}, frames); // Grok takes a start frame only
    expect(r.refs).toEqual(refs(1, "startFrame"));
    expect(r.note).toBe("Grok Imagine Video 1.5 Lite takes no end frame, so it was left out.");
    expect(carry(flare, veo, {}, {}, refs(2)).note).toBe("Veo 3.1 takes no added pictures, so 2 were left out.");
    expect(carry(flare, veo, {}, {}, refs(1, null)).note).toBe("Veo 3.1 takes no added picture, so it was left out."); // no role: a reference
  });

  it("drops an extra field the new model does not accept", () => {
    const r = carry(flare, veo, {}, { foo: 1 }, []);
    expect(r.extra).toEqual({});
    expect(r.note).toBe("Veo 3.1 does not accept foo, so it was removed.");
    expect(carry(flare, flare, {}, { complexity: 2, foo: 1 }, []).extra).toEqual({ complexity: 2 });
  });

  it("fills the new model's starting values for what it leaves unset", () => {
    expect(carry(flare, veo, {}, {}, []).values).toEqual(defaultsFor(veo));
    const r = carry(veo, grokLite, { duration: 8 }, {}, []);
    expect(r.values).toEqual({ duration: 8, resolution: "720p" });
    expect(r.note).toBeNull();
  });

  it("moves a value with no nearest one to the starting value where there is one", () => {
    const withAuto = withKind(veo, "resolution", { type: "enum", values: [{ value: "auto", label: "Auto" }] });
    const r = carry(withAuto, grokLite, { resolution: "auto" }, {}, []);
    expect(r.values.resolution).toBe("720p");
    expect(r.note).toBe("Resolution auto became 720p.");
  });

  it("matches a value whose key the old model does not declare by the same key", () => {
    expect(carry(null, klein, { seed: 5, steps: 8, size: "1024x768" }, {}, []).values).toEqual({ seed: 5, steps: 8, size: "1024x768" });
    const r = carry(null, veo, { duration: 5, foo_bar: 1 }, {}, []);
    expect(r.values).toEqual({ duration: 4, resolution: "720p" });
    expect(r.note).toBe("Length 5 s became 4 s. Veo 3.1 has no foo bar setting, so it was removed.");
  });

  it("without the old model, writes the note in the new model's field order", () => {
    const r = carry(null, klein, { steps: 60, aspect_ratio: "21:9" }, {}, []); // klein: Shape before Steps
    expect(r.note).toBe("Shape 21:9 became 16:9. Steps 60 became 50.");
  });

  it("leaves a name whose first word is in capitals as it is", () => {
    const cfg = { ...klein, params: klein.params.map((p) => (p.key === "steps" ? { ...p, label: "CFG scale" } : p)) };
    expect(carry(cfg, k3Edit, { steps: 3 }, {}, []).note).toBe("K3 Edit has no CFG scale setting, so it was removed.");
  });

  it("keeps an exact size only when the new model can make it", () => {
    expect(carry(null, klein, { size: "1000x1000" }, {}, []).note).toBe(
      "FLUX.2 klein 4B does not offer exact size 1000x1000, so it was removed.",
    ); // not a multiple of 16
    expect(carry(veo, seedance, { size: "1280x720" }, {}, []).values.size).toBe("1280x720");
  });

  it("carries between a cloud model and a local one by role", () => {
    const local = LOCAL.installed[0]!.model; // FLUX.2 klein 4B: shape, how many, references; seed
    const r = carry(flare, local, { aspect_ratio: "3:2", n: 4, quality: "high", seed: 81723 }, { moderation: "low" }, []);
    expect(r.values).toEqual({ aspect_ratio: "4:3", n: 4, seed: 81723 });
    expect(r.extra).toEqual({});
    expect(r.note).toBe(
      "Shape 3:2 became 4:3. FLUX.2 klein 4B has no quality setting, so it was removed. FLUX.2 klein 4B does not accept moderation, so it was removed.",
    );
  });

  it("writes the note in the old model's field order", () => {
    const r = carry(flare, klein, { background: "transparent", aspect_ratio: "21:9" }, { moderation: "low" }, refs(6));
    expect(r.note).toBe(
      "Shape 21:9 became 16:9. Only 4 added pictures fit, so 2 were left out. FLUX.2 klein 4B has no background setting, so it was removed. FLUX.2 klein 4B does not accept moderation, so it was removed.",
    );
  });
});

describe("defaultsFor", () => {
  it("defaults give a cheap first video", () => {
    expect(defaultsFor(veo)).toEqual({ duration: 4, resolution: "720p" });
  });

  it("gives a local video 2 s at 480p, and a local picture no Advanced starting value", () => {
    expect(defaultsFor(localVideo)).toEqual({ resolution: "480p", duration: 2 });
    expect(defaultsFor(klein)).toEqual({}); // steps starts at 4 under Advanced: not a change
  });
});

describe("changedCount", () => {
  it("counts Advanced changes", () => {
    expect(changedCount(flare, { output_format: "png", background: "transparent", quality: "high" }, {})).toBe(2); // quality is simple
    expect(changedCount(flare, {}, { moderation: "low" })).toBe(1);
    expect(changedCount(flare, {}, {})).toBe(0);
  });

  it("names the Advanced fields set, in the model's order, then the Extra fields", () => {
    const values = { background: "transparent", quality: "high", output_format: "png", foo: 1 }; // foo: no such field
    expect(changedKeys(flare, values, { moderation: "low" })).toEqual({
      values: ["output_format", "background"],
      extra: ["moderation"],
    });
    expect(changedCount(flare, values, { moderation: "low" })).toBe(3);
  });
});

describe("closestModel", () => {
  it("offers the newest model of the same family for a retired one", () => {
    const newer = { ...veo, id: "google/veo-3.2", created: veo.created! + 1 };
    expect(closestModel([veo, newer, grokLite], { id: "google/veo-3.0", family: "google/veo" })?.id).toBe("google/veo-3.2");
    expect(closestModel([grokLite], { id: "google/veo-3.0", family: "google/veo" })).toBeNull();
  });

  it("never offers the retired model itself", () => {
    const newer = { ...veo, id: "google/veo-3.2", created: veo.created! + 1 };
    expect(closestModel([veo, newer], { id: "google/veo-3.2", family: "google/veo" })?.id).toBe("google/veo-3.1");
    expect(closestModel([veo], { id: "google/veo-3.1", family: "google/veo" })).toBeNull();
  });

  it("never offers a model Studio cannot use", () => {
    const unusable = { ...veo, id: "google/veo-3.2", created: veo.created! + 1, usable: "No key" };
    expect(closestModel([veo, unusable], { id: "google/veo-3.0", family: "google/veo" })?.id).toBe("google/veo-3.1");
  });
});

describe("familyOf", () => {
  it("agrees with the Rust family rule", () => {
    expect(familyOf("google/veo-3.1-fast")).toBe("google/veo");
    expect(familyOf("kwaivgi/kling-v3.0-pro")).toBe("kwaivgi/kling");
    expect(familyOf("openai/gpt-image-2.6-flare")).toBe("openai/gpt-image");
    expect(familyOf("gpt-image-2.6-flare")).toBe("gpt-image");
    expect(familyOf("meta/muse-image")).toBe("meta/muse-image");
  });
});

describe("setting words", () => {
  it("lower-cases a word's first letter mid-sentence, unless its first word has another capital", () => {
    expect(SETTING_WORDS.midSentence("Extra high")).toBe("extra high");
    expect(SETTING_WORDS.midSentence("Transparent")).toBe("transparent");
    expect(SETTING_WORDS.midSentence("PNG")).toBe("PNG");
    expect(SETTING_WORDS.midSentence("WebP")).toBe("WebP");
    expect(SETTING_WORDS.midSentence("CFG scale")).toBe("CFG scale");
    expect(SETTING_WORDS.midSentence("DPM++ 2M")).toBe("DPM++ 2M");
    expect(SETTING_WORDS.midSentence("16:9")).toBe("16:9");
  });

  it("writes a number with its unit, and a key with spaces", () => {
    expect(SETTING_WORDS.withUnit("80", "%")).toBe("80%");
    expect(SETTING_WORDS.withUnit("120", "ms")).toBe("120 ms");
    expect(SETTING_WORDS.keyName("foo_bar")).toBe("foo bar");
  });
});

// Final review, area 3, I1: a model switch with LoRAs attached kept none and said
// "[object Object]". The LoRAs that fit the new model by `lorasFor`'s rule stay; each that does
// not is named in the note.
describe("carrying LoRAs", () => {
  const sdxl = byId("hf:stabilityai/stable-diffusion-xl-base-1.0:sd_xl_base_1.0.safetensors");
  const lora = (id: string, name: string, base: SdVersion | null): Lora => ({
    id,
    name,
    base,
    format: "kohya",
    sizeBytes: 1_000_000,
    source: "import",
    repo: null,
    missing: false,
  });
  const pixel = lora("local:a", "pixel art ü", "sdxl");
  const style = lora("local:b", "klein style", "flux2-klein");
  const mystery = lora("local:c", "mystery", null);
  const installed = [pixel, style, mystery];
  const uses: LoraUse[] = [
    { id: pixel.id, weight: 0.8, highNoise: false },
    { id: style.id, weight: 1, highNoise: false },
    { id: mystery.id, weight: 1.5, highNoise: true },
  ];

  it("keeps the LoRAs that fit the new model and names each one it removes", () => {
    const r = carry(klein, sdxl, { loras: uses }, {}, [], { loras: installed, base: "sdxl" });
    // The one made for SDXL and the one of no known type stay; the high-noise switch goes,
    // as sd_xl_base_1.0 has no high-noise part.
    expect(r.values.loras).toEqual([
      { id: pixel.id, weight: 0.8, highNoise: false },
      { id: mystery.id, weight: 1.5, highNoise: false },
    ]);
    expect(r.note).toBe("The LoRA klein style was made for FLUX.2 klein models, so it was removed.");
  });

  it("keeps every LoRA when the new model's type is not known, with no note", () => {
    const r = carry(klein, sdxl, { loras: uses }, {}, [], { loras: installed, base: null });
    expect((r.values.loras as LoraUse[]).map((u) => u.id)).toEqual([pixel.id, style.id, mystery.id]);
    expect(r.note).toBeNull();
  });

  it("names each LoRA removed for a model that takes none", () => {
    const r = carry(klein, flare, { loras: uses }, {}, [], { loras: installed, base: null });
    expect(r.values.loras).toBeUndefined();
    expect(r.note).toBe(
      "GPT Image 2.6 Flare takes no LoRAs, so pixel art ü, klein style and mystery were removed.",
    );
  });

  it("keeps as many as the new model takes, within its strengths", () => {
    const one = withKind(sdxl, "loras", {
      type: "loras",
      max: 1,
      highNoise: false,
      weightMin: 0,
      weightMax: 1,
      weightStep: 0.25,
      weightDefault: 1,
    });
    const r = carry(klein, one, { loras: [uses[2], uses[0]] }, {}, [], { loras: installed, base: "sdxl" });
    expect(r.values.loras).toEqual([{ id: mystery.id, weight: 1, highNoise: false }]);
    expect(r.note).toBe("sd_xl_base_1.0 takes up to 1 LoRA, so pixel art ü was removed.");
  });

  it("never writes a list or an object as text", () => {
    expect(valueWords([{ id: "x" }, { id: "y" }])).not.toContain("[object");
    expect(valueWords({ id: "x" })).not.toContain("[object");
    // Without the LoRAs installed it still says something true.
    const r = carry(klein, flare, { loras: uses }, {}, []);
    expect(r.note).not.toContain("[object");
    expect(r.note).toBe("GPT Image 2.6 Flare takes no LoRAs, so 3 LoRAs were removed.");
  });
});
