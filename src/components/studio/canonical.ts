// Carrying a draft's settings from one model to another (0.1.0 Task B4; studio-design 3).
// A value goes to the field that means the same thing on the new model (its `canonical`
// role, so `aspect_ratio` on one model fills the Shape field of another, cloud or local),
// moved to the nearest value that model takes, and one note names everything that changed.
// Also: the values Studio starts a model with, which fields show (what each requires is met)
// and the values that no longer apply, which Advanced settings are changed (counted in its
// header, a chip each under the prompt), and model families (the same rule as Rust's
// `family_of`) for offering a model in place of a retired one. The words are in words.ts.
import { CARRY_WORDS, SETTING_WORDS, STUDIO_TIME } from "../../lib/words";
import type { Choice, Layer, MediaModel, ParamSpec, RefItem, RefRole, SizeRules } from "./types";

/** A draft's settings, by the model's field keys; an unset field has no key. */
export type Values = Record<string, unknown>;

/** Quality names from cheapest to dearest; `auto` is kept only where it is offered. */
const QUALITY_LADDER = ["low", "medium", "high", "xhigh", "max"];

function isSet(value: unknown): boolean {
  return value !== undefined && value !== null;
}

/** The value Studio starts a field with: a Simple field's `initial` (a video's length and
 *  resolution). An Advanced starting value is not applied, so it never reads as a change. */
function startingValue(field: ParamSpec): unknown {
  return field.layer === "simple" && isSet(field.initial) ? field.initial : undefined;
}

/** The values Studio starts `model` with: the Simple fields' `initial` values only. */
export function defaultsFor(model: MediaModel): Values {
  const values: Values = {};
  for (const field of model.params) {
    const start = startingValue(field);
    if (start !== undefined) values[field.key] = start;
  }
  return values;
}

/** Whether a field holds a value in `Values` (added pictures and Extra fields are kept apart). */
function holdsValue(field: ParamSpec): boolean {
  return field.kind.type !== "images" && field.kind.type !== "extra";
}

/**
 * Carries a draft from `from` (`null` when its model is not known) to `to`. Each value goes to
 * the field of `to` with the same `canonical` role; a value whose key `from` does not declare,
 * or whose field has no role, goes to the field with the same key. Shape moves to the nearest
 * ratio, how many and other numbers into the field's range, quality and resolution keep their
 * name or move to the next cheaper one offered (the cheapest when none is cheaper), length to
 * the nearest (the shorter on a tie); anything else `to` does not offer is removed. Added
 * pictures are kept, in order, up to `to`'s limit for their role; Extra fields only where `to`
 * accepts them. Every key left unset is filled from `defaultsFor(to)`. The note has one
 * sentence for each change, in the order of `from`'s fields (`to`'s when `from` is not known),
 * or `null` when nothing changed.
 */
export function carry(
  from: MediaModel | null,
  to: MediaModel,
  values: Values,
  extra: Values,
  refs: RefItem[],
): { values: Values; extra: Values; refs: RefItem[]; note: string | null } {
  const model = to.displayName;
  const notes: { at: number; text: string }[] = [];
  const order = (from ?? to).params;
  /** Where a sentence goes: at the first field of `order` that `pick` matches, else last. */
  const at = (pick: (field: ParamSpec) => boolean) => {
    const index = order.findIndex(pick);
    return index < 0 ? Infinity : index;
  };

  const carried: Values = {};
  for (const [key, value] of Object.entries(values)) {
    if (!isSet(value)) continue;
    const source = from?.params.find((f) => f.key === key);
    const role = source?.canonical ?? null;
    const target = to.params.find((f) => holdsValue(f) && (role ? f.canonical === role : f.key === key));
    const here = at((f) => f.key === key);
    if (!target) {
      notes.push({ at: here, text: CARRY_WORDS.noSetting(model, source ? nameOf(source) : SETTING_WORDS.keyName(key)) });
      continue;
    }
    const placed = snap(target, value) ?? startingValue(target);
    const was = valueWords(value, source, target);
    if (placed === undefined) {
      notes.push({ at: here, text: CARRY_WORDS.notOffered(model, nameOf(target), was) });
      continue;
    }
    carried[target.key] = placed;
    if (placed !== value) {
      const now = valueWords(placed, target);
      notes.push({ at: here, text: CARRY_WORDS.became(target.label, SETTING_WORDS.midSentence(was), SETTING_WORDS.midSentence(now)) });
    }
  }
  for (const [key, start] of Object.entries(defaultsFor(to))) {
    if (!(key in carried)) carried[key] = start;
  }

  const kept: RefItem[] = [];
  const byRole = new Map<RefRole, number>();
  for (const ref of refs) {
    const role = ref.role ?? "reference";
    const count = (byRole.get(role) ?? 0) + 1;
    byRole.set(role, count);
    if (count <= pictureLimit(to, role)) kept.push(ref);
  }
  for (const [role, count] of byRole) {
    const fit = pictureLimit(to, role);
    if (count > fit) {
      const here = at((f) => f.kind.type === "images" && f.kind.role === role);
      notes.push({ at: here, text: CARRY_WORDS.leftOut(model, role, fit, count - fit) });
    }
  }

  const accepted = new Set(to.params.flatMap((f) => (f.kind.type === "extra" ? f.kind.allowedKeys : [])));
  const extraKept: Values = {};
  for (const [key, value] of Object.entries(extra)) {
    if (accepted.has(key)) extraKept[key] = value;
    else notes.push({ at: at((f) => f.kind.type === "extra"), text: CARRY_WORDS.notAccepted(model, key) });
  }

  // A stable sort: sentences for one field, and those after every field, keep their order.
  notes.sort((a, b) => (a.at === b.at ? 0 : a.at < b.at ? -1 : 1));
  const note = notes.map((n) => n.text).join(" ");
  return { values: carried, extra: extraKept, refs: kept, note: note || null };
}

/** How many pictures of `role` the model takes (0 when it has no field for them). */
function pictureLimit(model: MediaModel, role: RefRole): number {
  for (const field of model.params) {
    if (field.kind.type === "images" && field.kind.role === role) return field.kind.max;
  }
  return 0;
}

/** A field's name in the middle of a sentence ("Seed" → "seed", "How many" → "how many",
 *  "CFG scale" as it is). */
export function nameOf(field: ParamSpec): string {
  return SETTING_WORDS.midSentence(field.label);
}

function choicesOf(field: ParamSpec): Choice[] {
  return field.kind.type === "enum" || field.kind.type === "size" ? field.kind.values : [];
}

/** A value as the first of `fields` that lists it shows it ("7 s", "Extra high"), else with
 *  the field's unit, else as it is. */
export function valueWords(value: unknown, ...fields: (ParamSpec | undefined)[]): string {
  for (const field of fields) {
    const choice = field && choicesOf(field).find((c) => c.value === value);
    if (choice) return choice.label;
  }
  const field = fields.find((f) => f !== undefined);
  if (field?.kind.type === "range" && field.kind.unit) return SETTING_WORDS.withUnit(String(value), field.kind.unit);
  if (field?.canonical === "length" && typeof value === "number") return STUDIO_TIME.seconds(value);
  return String(value);
}

/** `value` as `field` takes it: the same, or the nearest it offers; `undefined` when none. */
function snap(field: ParamSpec, value: unknown): unknown {
  const kind = field.kind;
  switch (kind.type) {
    case "enum":
      return snapChoice(field, kind.values, value);
    case "range": {
      if (typeof value !== "number" || !Number.isFinite(value)) return undefined;
      const clamped = Math.min(kind.max, Math.max(kind.min, value));
      if (!kind.step) return clamped;
      const stepped = kind.min + Math.round((clamped - kind.min) / kind.step) * kind.step;
      return Math.min(kind.max, Number(stepped.toPrecision(12)));
    }
    case "boolean":
      return typeof value === "boolean" ? value : undefined;
    case "seed":
      return typeof value === "number" && Number.isInteger(value) && value >= 0 && value <= kind.max ? value : undefined;
    case "size":
      return offered(kind.values, value) ?? (typeof value === "string" && sizeFits(value, kind.rules) ? value : undefined);
    case "text":
      return typeof value === "string" && Array.from(value).length <= kind.maxChars ? value : undefined;
    default:
      return undefined;
  }
}

function offered(choices: Choice[], value: unknown): unknown {
  return choices.find((c) => c.value === value)?.value;
}

function snapChoice(field: ParamSpec, choices: Choice[], value: unknown): unknown {
  const same = offered(choices, value);
  if (same !== undefined) return same;
  switch (field.canonical) {
    case "shape":
      return nearestShape(choices, value);
    case "length":
      return nearestLength(choices, value);
    case "quality":
      return sameOrCheaper(choices, value, (v) => {
        const step = QUALITY_LADDER.indexOf(String(v));
        return step < 0 ? undefined : step;
      });
    case "resolution":
      return sameOrCheaper(choices, value, (v) => resolutionLines(String(v)));
    default:
      return undefined;
  }
}

/** A `W:H` shape as a ratio; `undefined` for anything else (`auto`). */
function ratioOf(value: unknown): number | undefined {
  const match = /^(\d+(?:\.\d+)?):(\d+(?:\.\d+)?)$/.exec(String(value));
  if (!match) return undefined;
  const ratio = Number(match[1]) / Number(match[2]);
  return Number.isFinite(ratio) && ratio > 0 ? ratio : undefined;
}

/** The offered ratio nearest `value` by |ln(w/h) − ln(w′/h′)|, the first on a tie. */
function nearestShape(choices: Choice[], value: unknown): unknown {
  const want = ratioOf(value);
  if (want === undefined) return undefined;
  let best: unknown;
  let bestGap = Infinity;
  for (const choice of choices) {
    const ratio = ratioOf(choice.value);
    if (ratio === undefined) continue;
    const gap = Math.abs(Math.log(ratio) - Math.log(want));
    if (gap < bestGap) {
      best = choice.value;
      bestGap = gap;
    }
  }
  return best;
}

/** The offered length nearest `value`, the shorter on a tie. */
function nearestLength(choices: Choice[], value: unknown): unknown {
  if (typeof value !== "number") return undefined;
  let best: number | undefined;
  for (const { value: length } of choices) {
    if (typeof length !== "number") continue;
    const gap = Math.abs(length - value);
    const bestGap = best === undefined ? Infinity : Math.abs(best - value);
    if (gap < bestGap || (gap === bestGap && best !== undefined && length < best)) best = length;
  }
  return best;
}

/** The dearest offered value no dearer than `value` by `rank`, else the cheapest offered. */
function sameOrCheaper(choices: Choice[], value: unknown, rank: (v: unknown) => number | undefined): unknown {
  const want = rank(value);
  if (want === undefined) return undefined;
  let below: { value: unknown; rank: number } | undefined;
  let lowest: { value: unknown; rank: number } | undefined;
  for (const choice of choices) {
    const r = rank(choice.value);
    if (r === undefined) continue;
    if (r <= want && (!below || r > below.rank)) below = { value: choice.value, rank: r };
    if (!lowest || r < lowest.rank) lowest = { value: choice.value, rank: r };
  }
  return (below ?? lowest)?.value;
}

/** A resolution's lines by the number in its name (a resolution's value is its label):
 *  `720p` → 720, `1K` → 1024, `2K` → 2048, and `4K` → 2160, UHD's lines, so it ranks with
 *  `2160p` above `2K`. */
function resolutionLines(name: string): number | undefined {
  const match = /^(\d+(?:\.\d+)?)\s*([kKpP]?)$/.exec(name.trim());
  if (!match) return undefined;
  const n = Number(match[1]);
  if (match[2]?.toLowerCase() !== "k") return n;
  return n === 4 ? 2160 : n * 1024;
}

/** Whether a custom `WxH` size meets `rules` (the same checks as Rust's `check_size`). */
function sizeFits(size: string, rules: SizeRules): boolean {
  const match = /^(\d+)x(\d+)$/.exec(size);
  if (!match) return false;
  const width = Number(match[1]);
  const height = Number(match[2]);
  if (width <= 0 || height <= 0) return false;
  if (rules.multipleOf > 0 && (width % rules.multipleOf !== 0 || height % rules.multipleOf !== 0)) return false;
  const ratio = width / height;
  if (ratio < rules.minRatio - 1e-9 || ratio > rules.maxRatio + 1e-9) return false;
  if (Math.max(width, height) > rules.maxSide) return false;
  const pixels = width * height;
  return pixels >= rules.minPixels && pixels <= rules.maxPixels;
}

/** Whether what `spec` requires is met: the field it names has one of the values listed, an
 *  unset field counting as its company default (the backend's `unmet` rule). */
function applies(params: ParamSpec[], spec: ParamSpec, values: Values): boolean {
  const requires = spec.requires;
  if (!requires) return true;
  const set = values[requires.key];
  const current = isSet(set) ? set : (params.find((p) => p.key === requires.key)?.companyDefault ?? null);
  return current !== null && requires.values.some((v) => v === current);
}

/** The fields of `layer`, in the model's order, whose requirements `values` meets. */
export function visibleFields(params: ParamSpec[], layer: Layer, values: Values): ParamSpec[] {
  return params.filter((p) => p.layer === layer && applies(params, p, values));
}

/** `values` without each value whose field requires what `values` no longer has, again until
 *  nothing more goes (a field may require one that went). The same object when nothing goes. */
export function dropUnmet(model: MediaModel, values: Values): Values {
  let now = values;
  for (;;) {
    const gone = new Set(
      Object.keys(now).filter((key) => {
        const spec = model.params.find((p) => p.key === key);
        return spec !== undefined && !applies(model.params, spec, now);
      }),
    );
    if (gone.size === 0) return now;
    now = Object.fromEntries(Object.entries(now).filter(([key]) => !gone.has(key)));
  }
}

/** The Advanced settings `values` and `extra` change: the key of each Advanced field set, in
 *  the model's order, and each Extra field. A Simple field (quality, say) is not one. */
export function changedKeys(model: MediaModel, values: Values, extra: Values): { values: string[]; extra: string[] } {
  const advanced = model.params.filter((f) => f.layer === "advanced" && holdsValue(f) && isSet(values[f.key]));
  return { values: advanced.map((f) => f.key), extra: Object.keys(extra) };
}

/** How many Advanced settings `values` and `extra` change (`changedKeys`): Advanced's header
 *  counts them, and each is a chip under the prompt. */
export function changedCount(model: MediaModel, values: Values, extra: Values): number {
  const changed = changedKeys(model, values, extra);
  return changed.values.length + changed.extra.length;
}

/**
 * A model's family: its maker and its name without the version, so versions of one model
 * group together (`google/veo-3.1-fast` → `google/veo`). The same rule as Rust's `family_of`:
 * the name is cut before the first `-` followed by a digit, or by `v` and a digit.
 */
export function familyOf(id: string): string {
  const slash = id.indexOf("/");
  const maker = slash < 0 ? null : id.slice(0, slash);
  const name = slash < 0 ? id : id.slice(slash + 1);
  const isDigit = (c: string | undefined) => c !== undefined && c >= "0" && c <= "9";
  let cut = name.length;
  for (let i = name.indexOf("-"); i >= 0; i = name.indexOf("-", i + 1)) {
    const next = name[i + 1];
    if (isDigit(next) || (next === "v" && isDigit(name[i + 2]))) {
      cut = i;
      break;
    }
  }
  return maker === null ? name.slice(0, cut) : `${maker}/${name.slice(0, cut)}`;
}

/** The model to offer for a retired one: the newest usable model of its family other than
 *  itself (by `created`; one without a date counts as the oldest), or `null` when there is none. */
export function closestModel(models: MediaModel[], retired: { id: string; family: string }): MediaModel | null {
  let best: MediaModel | null = null;
  for (const model of models) {
    if (model.id === retired.id || model.family !== retired.family || model.usable !== null) continue;
    if (!best || (model.created ?? -Infinity) > (best.created ?? -Infinity)) best = model;
  }
  return best;
}
