import { useEffect, useId, useState } from "react";
import { X } from "lucide-react";
import { LOCAL_MARK, STUDIO_FIELDS, STUDIO_PLACE, STUDIO_SETUPS } from "../../lib/words";
import { toErrorInfo } from "../../store/app";
import { draftModel, localFitKey, useStudio, type Draft, type StudioTab } from "../../store/studio";
import { AdvancedDrawer } from "./AdvancedDrawer";
import { visibleFields, type Values } from "./canonical";
import { etaText } from "./eta";
import { ModelPicker } from "./ModelPicker";
import { ParamField } from "./ParamField";
import { FrameDrop } from "./RefTray";
import type { Canonical, MediaModel, ParamSpec, Setup } from "./types";

// Studio's right panel (0.1.0 Task B5; design Wireframe 1, local addendum "Right panel"): the
// Model field (ModelPicker.tsx) with where the model runs ("On this computer · <device>" or
// "via <endpoint> · Cloud"), the note of what changed, the model's simple fields in its own
// order (a frame as a drop zone; reference pictures go in the tray), My setups and the Advanced
// drawer with its change count (Task B6, AdvancedDrawer.tsx). On this computer, each Length
// and Resolution chip of a video shows its own time here, asked once per chip and size
// (nothing is sent).

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
  const setValue = useStudio((s) => s.setValue);
  const resetValue = useStudio((s) => s.resetValue);
  const dismissNote = useStudio((s) => s.dismissNote);
  const askLocalFit = useStudio((s) => s.askLocalFit);
  const here = draft.target === "thisComputer";
  const localVideo = here && model?.kind === "video";

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
      <MySetups tab={tab} />
      <AdvancedDrawer tab={tab} />
    </div>
  );
}

const BOX =
  "rounded-md border border-border bg-bg px-2 py-1 text-sm outline-none focus:border-border-strong disabled:opacity-60";
const SMALL_BUTTON = "rounded-md px-1.5 py-0.5 text-xs text-fg-muted hover:bg-bg-muted hover:text-fg";

/** While the preferences hold no setups. */
const NO_SETUPS: Setup[] = [];

/**
 * My setups: a menu of the tab's saved setups, which applies the one picked (or says why it
 * cannot), and *Save as setup…*, which keeps the model and every setting under a name. The menu
 * names the setup applied or saved last until the draft changes, then None.
 */
function MySetups(props: { tab: StudioTab }) {
  const { tab } = props;
  const all = useStudio((s) => s.prefs?.setups) ?? NO_SETUPS;
  const draft = useStudio((s) => s.drafts[tab]);
  const applySetup = useStudio((s) => s.applySetup);
  const saveSetup = useStudio((s) => s.saveSetup);
  const menuId = useId();
  // The setup the draft was set from, with the draft it gave.
  const [from, setFrom] = useState<{ name: string; draft: Draft } | null>(null);
  // The name being typed (`null` while not saving), and why a pick or a save did not happen.
  const [name, setName] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const setups = all.filter((s) => s.kind === tab);
  const shown = from !== null && from.draft === draft ? setups.findIndex((s) => s.name === from.name) : -1;

  const pick = (value: string) => {
    const setup = value === "" ? undefined : setups[Number(value)];
    if (!setup) return;
    const refused = applySetup(setup);
    setMessage(refused);
    setFrom(refused === null ? { name: setup.name, draft: useStudio.getState().drafts[tab] } : null);
  };
  const save = async () => {
    if (name === null || name.trim() === "" || saving) return;
    setSaving(true);
    try {
      const refused = await saveSetup(tab, name);
      setMessage(refused);
      if (refused === null) {
        setFrom({ name: name.trim(), draft: useStudio.getState().drafts[tab] });
        setName(null);
      }
    } catch (e: unknown) {
      setMessage(toErrorInfo(e).message);
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-center gap-2">
        <label htmlFor={menuId} className="text-xs font-medium text-fg-muted">
          {STUDIO_SETUPS.label}
        </label>
        <select
          id={menuId}
          value={shown < 0 ? "" : String(shown)}
          onChange={(e) => pick(e.target.value)}
          className={`${BOX} min-w-0 flex-1`}
        >
          <option value="">{STUDIO_SETUPS.none}</option>
          {setups.map((setup, i) => (
            <option key={`${i} ${setup.name}`} value={String(i)}>
              {setup.name}
            </option>
          ))}
        </select>
      </div>
      {name === null ? (
        <button
          type="button"
          onClick={() => {
            setName("");
            setMessage(null);
          }}
          className={`${SMALL_BUTTON} self-start`}
        >
          {STUDIO_SETUPS.saveAs}
        </button>
      ) : (
        <div className="flex items-center gap-1.5">
          <input
            aria-label={STUDIO_SETUPS.name}
            placeholder={STUDIO_SETUPS.name}
            value={name}
            autoFocus
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                void save();
              }
            }}
            className={`${BOX} min-w-0 flex-1`}
          />
          <button
            type="button"
            disabled={saving || name.trim() === ""}
            onClick={() => void save()}
            className="rounded-md border border-border px-2 py-0.5 text-xs whitespace-nowrap hover:border-border-strong disabled:opacity-60"
          >
            {STUDIO_SETUPS.save}
          </button>
          <button
            type="button"
            onClick={() => {
              setName(null);
              setMessage(null);
            }}
            className={SMALL_BUTTON}
          >
            {STUDIO_SETUPS.cancel}
          </button>
        </div>
      )}
      {message && <p className="text-xs text-warning">{message}</p>}
    </div>
  );
}
