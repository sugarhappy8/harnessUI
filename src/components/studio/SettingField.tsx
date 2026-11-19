import { draftModel, localFitKey, useStudio, type StudioTab } from "../../store/studio";
import type { Values } from "./canonical";
import { etaText } from "./eta";
import { ParamField } from "./ParamField";
import { FrameDrop } from "./RefTray";
import type { Canonical, LocalFit, MediaModel, ParamSpec } from "./types";

// One field of Studio's right panel, whichever layer draws it (0.1.0 Tasks B5 and B6): the
// simple fields (SettingsPanel.tsx) and the Advanced drawer (AdvancedDrawer.tsx) both draw each
// field here, so a field looks and works the same in either. A frame is its drop zone (reference
// pictures are added in the tray beside the prompt); anything else goes through ParamField with
// its reset, the seed with its dice and *Keep it*, and on this computer each Length and
// Resolution chip of a clip with its own time here. Those times are asked for by the panel
// (`clipAsks`, asked once per chip and size; nothing is sent) and read here.

/** A `W:H` shape's two sides; `null` for anything else (`auto`). */
function shapeOf(value: unknown): { w: number; h: number } | null {
  const match = typeof value === "string" ? /^(\d+):(\d+)$/.exec(value) : null;
  const w = Number(match?.[1]);
  const h = Number(match?.[2]);
  return match && w > 0 && h > 0 ? { w, h } : null;
}

/** A resolution's lines by its name: "480p" → 480; `null` for anything else. */
function linesOf(value: unknown): number | null {
  const match = typeof value === "string" ? /^(\d+)p$/i.exec(value.trim()) : null;
  return match ? Number(match[1]) : null;
}

function fieldOf(model: MediaModel, role: Canonical): ParamSpec | undefined {
  return model.params.find((p) => p.canonical === role);
}

/** A field's value as set, else `fallback`. */
function valueOr(values: Values, field: ParamSpec | undefined, fallback: unknown): unknown {
  const set = field ? values[field.key] : undefined;
  return set !== undefined && set !== null ? set : fallback;
}

/**
 * The size a local clip runs at, as the backend works it out (`size_of` in studio/local.rs): an
 * Exact size as it is; else the shorter side at the resolution's lines (`resolution`, a chip's
 * own, else the draft's, else the Resolution field's starting value) and the longer at the
 * shape (the draft's, else the Shape field's company default), kept within the Exact size
 * field's longest side, both rounded down to its multiple. `null` when the model gives too
 * little to tell.
 */
export function localClipSize(model: MediaModel, values: Values, resolution?: string): { w: number; h: number } | null {
  const sizeField = fieldOf(model, "size");
  const exact = valueOr(values, sizeField, null);
  const sides = typeof exact === "string" ? /^(\d+)x(\d+)$/.exec(exact) : null;
  if (sides) return { w: Number(sides[1]), h: Number(sides[2]) };

  const shapeField = fieldOf(model, "shape");
  const shape = shapeOf(valueOr(values, shapeField, shapeField?.companyDefault));
  const resolutionField = fieldOf(model, "resolution");
  const lines = linesOf(resolution ?? valueOr(values, resolutionField, resolutionField?.initial));
  if (!shape || lines === null) return null;

  const rules = sizeField?.kind.type === "size" ? sizeField.kind.rules : null;
  const multiple = rules && rules.multipleOf > 0 ? rules.multipleOf : 1;
  const maxSide = rules?.maxSide ?? Infinity;
  const lo = Math.min(shape.w, shape.h);
  const hi = Math.max(shape.w, shape.h);
  let long = Math.floor((lines * hi) / lo);
  let short = lines;
  if (long > maxSide) {
    long = maxSide;
    short = Math.floor((maxSide * lo) / hi);
  }
  long = Math.floor(long / multiple) * multiple;
  short = Math.floor(short / multiple) * multiple;
  if (short <= 0) return null;
  return shape.w >= shape.h ? { w: long, h: short } : { w: short, h: long };
}

/** One chip's question for a local clip's time: its field, its value, and the size and length. */
export interface ClipAsk {
  role: "length" | "resolution";
  value: unknown;
  width: number;
  height: number;
  seconds: number;
}

/** Each Length chip at its own length and the draft's size; each Resolution chip at its own
 *  size and the draft's length (0 for the model's own). */
export function clipAsks(model: MediaModel, values: Values): ClipAsk[] {
  const asks: ClipAsk[] = [];
  const length = fieldOf(model, "length");
  const resolution = fieldOf(model, "resolution");
  const size = localClipSize(model, values);
  if (length?.kind.type === "enum" && size) {
    for (const { value } of length.kind.values) {
      if (typeof value === "number") asks.push({ role: "length", value, width: size.w, height: size.h, seconds: value });
    }
  }
  if (resolution?.kind.type === "enum") {
    const set = valueOr(values, length, 0);
    const seconds = typeof set === "number" ? set : 0;
    for (const { value } of resolution.kind.values) {
      const at = localClipSize(model, values, String(value));
      if (at) asks.push({ role: "resolution", value, width: at.w, height: at.h, seconds });
    }
  }
  return asks;
}

/** For a local clip's Length or Resolution field, each chip's time here from the answers in
 *  `localFits` (no time until its answer comes); `undefined` for any other field. */
function clipTimeOf(
  model: MediaModel,
  values: Values,
  localFits: Record<string, LocalFit>,
  spec: ParamSpec,
): ((value: unknown) => string | null) | undefined {
  const role = spec.canonical === "length" || spec.canonical === "resolution" ? spec.canonical : null;
  if (!role) return undefined;
  const asks = clipAsks(model, values);
  return (value) => {
    const ask = asks.find((a) => a.role === role && a.value === value);
    const fit = ask ? localFits[localFitKey(model.id, ask.width, ask.height, ask.seconds)] : undefined;
    return etaText(fit?.eta ?? null);
  };
}

/** Field `spec` of `tab`'s model, set and reset in `tab`'s draft. */
export function SettingField(props: { tab: StudioTab; spec: ParamSpec }) {
  const { tab, spec } = props;
  const draft = useStudio((s) => s.drafts[tab]);
  const model = useStudio((s) => draftModel(s, tab));
  const localFits = useStudio((s) => s.localFits);
  const setValue = useStudio((s) => s.setValue);
  const resetValue = useStudio((s) => s.resetValue);
  const newSeed = useStudio((s) => s.newSeed);
  const lockSeed = useStudio((s) => s.lockSeed);
  if (spec.kind.type === "images") {
    // Reference pictures are added in the tray beside the prompt.
    return spec.kind.role === "reference" ? null : <FrameDrop tab={tab} spec={spec} />;
  }
  const here = draft.target === "thisComputer";
  const localClip = here && model?.kind === "video";
  return (
    <ParamField
      spec={spec}
      value={draft.values[spec.key]}
      onChange={(v) => setValue(tab, spec.key, v)}
      onReset={() => resetValue(tab, spec.key)}
      local={here}
      timeOf={localClip && model ? clipTimeOf(model, draft.values, localFits, spec) : undefined}
      seed={
        spec.kind.type === "seed"
          ? { locked: draft.seedLocked, onNew: () => newSeed(tab), onLock: (locked) => lockSeed(tab, locked) }
          : undefined
      }
    />
  );
}
