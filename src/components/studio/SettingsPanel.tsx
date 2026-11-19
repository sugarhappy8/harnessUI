import { useEffect, useState } from "react";
import { ChevronDown, ChevronRight, X } from "lucide-react";
import { LOCAL_MARK, STUDIO_FIELDS, STUDIO_PLACE } from "../../lib/words";
import { draftModel, localFitKey, useStudio, type StudioTab } from "../../store/studio";
import { changedCount, visibleFields, type Values } from "./canonical";
import { etaText } from "./eta";
import { ModelPicker } from "./ModelPicker";
import { ParamField } from "./ParamField";
import { FrameDrop } from "./RefTray";
import type { Canonical, MediaModel, ParamSpec } from "./types";

// Studio's right panel (0.1.0 Task B5; design Wireframe 1, local addendum "Right panel"): the
// Model field (ModelPicker.tsx) with where the model runs ("On this computer · <device>" or
// "via <endpoint> · Cloud"), the note of what changed, the model's simple fields in its own
// order (a frame as a drop zone; reference pictures go in the tray), My setups (Task B6) and
// the Advanced toggle with its change count. On this computer, each Length and Resolution chip
// of a video shows its own time here, asked once per chip and size (nothing is sent).

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
interface ClipAsk {
  role: "length" | "resolution";
  value: unknown;
  width: number;
  height: number;
  seconds: number;
}

/** Each Length chip at its own length and the draft's size; each Resolution chip at its own
 *  size and the draft's length (0 for the model's own). */
function clipAsks(model: MediaModel, values: Values): ClipAsk[] {
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

/** The right panel of `tab`. */
export function SettingsPanel(props: { tab: StudioTab }) {
  const { tab } = props;
  const draft = useStudio((s) => s.drafts[tab]);
  const model = useStudio((s) => draftModel(s, tab));
  const local = useStudio((s) => s.local);
  const endpoints = useStudio((s) => s.endpoints);
  const localFits = useStudio((s) => s.localFits);
  const savedOpen = useStudio((s) => s.prefs?.advancedOpen ?? false);
  const setValue = useStudio((s) => s.setValue);
  const resetValue = useStudio((s) => s.resetValue);
  const dismissNote = useStudio((s) => s.dismissNote);
  const askLocalFit = useStudio((s) => s.askLocalFit);
  const setPrefs = useStudio((s) => s.setPrefs);
  const here = draft.target === "thisComputer";
  const localVideo = here && model?.kind === "video";
  // Whether Advanced is open: what was saved, and the toggle's own choice even when saving it
  // fails; a choice saved elsewhere since is followed.
  const [advancedOpen, setAdvancedOpen] = useState(savedOpen);
  const [shownSaved, setShownSaved] = useState(savedOpen);
  if (shownSaved !== savedOpen) {
    setShownSaved(savedOpen);
    setAdvancedOpen(savedOpen);
  }

  // A local clip's chips each ask for their time, once per size and length: the store keeps
  // the answers (and which could not be worked out) and asks only for what it has not got. It
  // forgets them when the local models are read again, so this runs again as `localFits`
  // changes and asks for them again.
  useEffect(() => {
    if (!model || !localVideo) return;
    for (const ask of clipAsks(model, draft.values)) askLocalFit(model.id, ask.width, ask.height, ask.seconds);
  }, [model, localVideo, draft.values, localFits, askLocalFit]);

  if (!model) {
    return (
      <div data-settings-panel className="flex flex-col gap-4 p-3">
        <ModelPicker tab={tab} />
      </div>
    );
  }

  const asks = localVideo ? clipAsks(model, draft.values) : [];
  /** A chip's time here, for a local clip's Length or Resolution field. */
  const timeFor = (spec: ParamSpec) => {
    const role = spec.canonical === "length" || spec.canonical === "resolution" ? spec.canonical : null;
    if (!localVideo || !role) return undefined;
    return (value: unknown) => {
      const ask = asks.find((a) => a.role === role && a.value === value);
      const fit = ask ? localFits[localFitKey(model.id, ask.width, ask.height, ask.seconds)] : undefined;
      return etaText(fit?.eta ?? null);
    };
  };

  const target = draft.target;
  const endpoint = target !== null && target !== "thisComputer" ? endpoints.find((e) => e.providerId === target.endpoint) : undefined;
  const place = here
    ? local
      ? STUDIO_PLACE.here(local.hardware.device)
      : LOCAL_MARK.name
    : endpoint
      ? STUDIO_PLACE.cloud(endpoint.name)
      : null;

  const draw = (spec: ParamSpec) => {
    if (spec.kind.type === "images") {
      // Reference pictures are added in the tray beside the prompt.
      return spec.kind.role === "reference" ? null : <FrameDrop key={spec.key} tab={tab} spec={spec} />;
    }
    return (
      <ParamField
        key={spec.key}
        spec={spec}
        value={draft.values[spec.key]}
        onChange={(v) => setValue(tab, spec.key, v)}
        onReset={() => resetValue(tab, spec.key)}
        local={here}
        timeOf={timeFor(spec)}
      />
    );
  };

  const hasAdvanced = model.params.some((p) => p.layer === "advanced");
  const changed = changedCount(model, draft.values, draft.extra);
  const toggleAdvanced = () => {
    const open = !advancedOpen;
    setAdvancedOpen(open);
    setPrefs({ advancedOpen: open }).catch((e: unknown) =>
      // It opens or closes all the same, for now; the failure leaves a trace.
      console.warn("Studio: whether Advanced is open could not be saved", e),
    );
  };

  return (
    <div data-settings-panel className="flex flex-col gap-4 p-3">
      <div className="flex flex-col gap-1">
        <ModelPicker tab={tab} />
        {place && <p className="text-xs text-fg-muted">{place}</p>}
      </div>
      {draft.note && (
        <div role="status" className="flex items-start gap-1.5 rounded-lg bg-bg-muted px-2 py-1.5 text-xs">
          <p className="min-w-0 flex-1">{draft.note}</p>
          <button
            type="button"
            aria-label={STUDIO_FIELDS.putNoteAway}
            title={STUDIO_FIELDS.putNoteAway}
            onClick={() => dismissNote(tab)}
            className="rounded p-0.5 text-fg-muted hover:text-fg"
          >
            <X size={12} aria-hidden />
          </button>
        </div>
      )}
      {visibleFields(model.params, "simple", draft.values).map(draw)}
      {/* My setups (Task B6). */}
      {hasAdvanced && (
        <div className="flex flex-col gap-4 border-t border-border pt-3">
          <button
            type="button"
            aria-expanded={advancedOpen}
            onClick={toggleAdvanced}
            className="flex items-center gap-1 self-start text-sm font-medium hover:text-fg"
          >
            {advancedOpen ? <ChevronDown size={14} aria-hidden /> : <ChevronRight size={14} aria-hidden />}
            {changed > 0 ? STUDIO_FIELDS.advancedChanged(changed) : STUDIO_FIELDS.advanced}
          </button>
          {advancedOpen && visibleFields(model.params, "advanced", draft.values).map(draw)}
        </div>
      )}
    </div>
  );
}
