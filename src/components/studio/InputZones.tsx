import { useId, useRef } from "react";
import { ImagePlus, X } from "lucide-react";
import { INPUT_ROLE, STUDIO_COMPOSER, STUDIO_REFS } from "../../lib/words";
import { useStudio, type StudioTab } from "../../store/studio";
import { FOCUS_RING } from "../ui/styles";
import { Tip } from "../ui/Tip";
import { dropTarget, fromDrop, fromFiles, PICTURE_TYPES, RefThumb, RefusalWords, useAdding, useNativeDropOn } from "./RefTray";
import { PICTURE_TILE } from "./styles";
import type { MediaModel, ParamSpec, RefRole } from "./types";

// The composer's input pictures (0.2.0 Task U3; design 11a): one zone per picture field of the
// model whose role is not a reference picture (the picture to change and its mask for
// inpainting, a control picture for Canny, Depth and Flex.2, a video's start and end frames).
// Each is a labelled drop target and button ("Add the picture to change") that takes one
// picture, dropped or chosen, a new one taking the place of the one there; once set it shows
// the picture with Remove. Its hint is a tooltip, and "required" marks a field whose least is
// one. Reference pictures are the strip's (RefTray.tsx), which shares the adding here.

/** The roles that have a zone of their own. */
const ZONE_ROLES: ReadonlySet<RefRole> = new Set<RefRole>(["source", "mask", "control", "startFrame", "endFrame"]);

/** What a zone's tooltip says: Studio's own hint for a mask or a control picture, else the
 *  field's help, if any. */
function hintOf(spec: ParamSpec, role: RefRole): string | null {
  if (role === "mask" || role === "control") return INPUT_ROLE.hint[role];
  return spec.help;
}

/** One zone per input picture field of `model`, in its order; nothing when it has none. */
export function InputZones(props: { tab: StudioTab; model: MediaModel }) {
  const zones = props.model.params.filter((p) => p.kind.type === "images" && ZONE_ROLES.has(p.kind.role));
  if (zones.length === 0) return null;
  return (
    // `contents`: each zone is an item of the composer's row, beside the strip.
    <div data-input-zones className="contents">
      {zones.map((spec) => (
        <InputZone key={spec.key} tab={props.tab} spec={spec} />
      ))}
    </div>
  );
}

/** The zone of picture field `spec` (one picture of its role) in `tab`'s draft. */
export function InputZone(props: { tab: StudioTab; spec: ParamSpec }) {
  const { tab, spec } = props;
  const role: RefRole = spec.kind.type === "images" ? spec.kind.role : "source";
  const required = spec.kind.type === "images" && spec.kind.min >= 1;
  const refs = useStudio((s) => s.drafts[tab].refs);
  const removeRef = useStudio((s) => s.removeRef);
  const { add, error } = useAdding(tab);
  const fileInput = useRef<HTMLInputElement>(null);
  const zone = useRef<HTMLButtonElement>(null);
  const labelId = useId();
  const index = refs.findIndex((r) => (r.role ?? "reference") === role);
  const item = index >= 0 ? refs[index] : undefined;
  const take = (files: File[]) => add(fromFiles(files.slice(0, 1)), role, true);
  useNativeDropOn(zone, () => add(fromDrop(1), role, true));
  const hint = hintOf(spec, role);

  // The button covers the whole tile, so the tile is one target to press or drop on; the
  // label and "required" are drawn under it, and Remove sits above it.
  const button = (
    <button
      ref={zone}
      type="button"
      data-input-zone
      aria-label={item ? STUDIO_REFS.change(spec.label) : STUDIO_COMPOSER.addInput(spec.label)}
      onClick={() => fileInput.current?.click()}
      className={`absolute inset-0 rounded-xl ${FOCUS_RING}`}
      {...dropTarget(take)}
    />
  );

  return (
    <>
      <div
        role="group"
        aria-labelledby={labelId}
        data-param={spec.key}
        className={`relative ${PICTURE_TILE} pr-3 ${item ? "border-border bg-surface pl-1" : "border-dashed border-border-strong pl-2.5 text-fg-muted hover:border-accent hover:text-fg"}`}
      >
        {item ? (
          <RefThumb sha256={item.sha256} mime={item.mime} alt={spec.label} className="size-10 rounded-lg" />
        ) : (
          <ImagePlus size={16} aria-hidden className="shrink-0" />
        )}
        <span className="flex flex-col leading-tight whitespace-nowrap">
          {/* Names the group; the button says what pressing it does. */}
          <span id={labelId} aria-hidden className={item ? "text-fg" : undefined}>
            {spec.label}
          </span>
          {required && <span className="text-xs text-fg-muted">{STUDIO_COMPOSER.required}</span>}
        </span>
        {hint ? <Tip content={hint}>{button}</Tip> : button}
        {item && (
          <button
            type="button"
            aria-label={STUDIO_COMPOSER.remove(spec.label)}
            onClick={() => removeRef(tab, index)}
            className={`relative -mr-1.5 rounded-full p-1 text-fg-muted hover:bg-bg-muted hover:text-fg ${FOCUS_RING}`}
          >
            <X size={14} aria-hidden />
          </button>
        )}
        <input
          ref={fileInput}
          type="file"
          accept={PICTURE_TYPES}
          hidden
          aria-label={STUDIO_REFS.chooseFor(spec.label)}
          onChange={(e) => {
            if (e.target.files) take([...e.target.files]);
            e.target.value = "";
          }}
        />
      </div>
      {error && (
        <p role="alert" className="basis-full text-xs text-danger">
          <RefusalWords error={error} />
        </p>
      )}
    </>
  );
}
