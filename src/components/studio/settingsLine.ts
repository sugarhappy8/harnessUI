import { STUDIO_CARD, STUDIO_CONFIRM, STUDIO_TIME } from "../../lib/words";
import { valueWords } from "./canonical";
import type { Canonical, MediaModel, Run } from "./types";

// The settings line of a run's card (0.1.0 Task B8; design Wireframe 3, local addendum
// Wireframe L4), built only from the run's own settings: "Veo 3.1 Fast · OpenRouter · 8 s ·
// 16:9 · 720p · sound on · start frame", or for a local run "FLUX.2 klein 4B · on this
// computer · 1024×1024 · ×2 · seed 81723". RunCard puts the model and where it ran first.

/** The settings line after the model and where it ran, from the run's own settings (the model
 *  as listed names their fields, else their usual keys): a cloud run's length, shape,
 *  resolution, size, sound, count, seed and added pictures; a local run's exact size (else its
 *  results', else its shape), length, resolution, count, seed and added pictures. */
export function settingsParts(run: Run, model: MediaModel | null): string[] {
  const field = (canonical: Canonical) => model?.params.find((p) => p.canonical === canonical);
  const value = (canonical: Canonical, key: string): unknown => {
    const v = run.params[field(canonical)?.key ?? key];
    return v === undefined || v === null || v === "" ? null : v;
  };
  const words = (canonical: Canonical, key: string): string | null => {
    const v = value(canonical, key);
    return v === null ? null : valueWords(v, field(canonical));
  };
  const size = (): string | null => {
    const v = value("size", "size");
    const sides = typeof v === "string" ? v.split("x") : [];
    return sides.length === 2 ? STUDIO_CARD.size(sides[0] ?? "", sides[1] ?? "") : v === null ? null : String(v);
  };
  const length = (): string | null => {
    const v = value("length", "duration");
    if (v === null) return null;
    return field("length") ? valueWords(v, field("length")) : typeof v === "number" ? STUDIO_TIME.seconds(v) : String(v);
  };
  const sound = value("sound", "generate_audio");
  const count = value("count", "n");
  const seed = value("seed", "seed");
  const refs: string[] = [];
  const references = run.refs.filter((r) => (r.role ?? "reference") === "reference").length;
  if (run.refs.some((r) => r.role === "startFrame")) refs.push(STUDIO_CARD.startFrame);
  if (run.refs.some((r) => r.role === "endFrame")) refs.push(STUDIO_CARD.endFrame);
  if (references > 0) refs.push(STUDIO_CARD.addedPictures(references));
  const output = run.outputs[0];
  const shown =
    run.runsOn === "local"
      ? [
          size() ??
            (output?.width != null && output.height != null
              ? STUDIO_CARD.size(String(output.width), String(output.height))
              : words("shape", "aspect_ratio")),
          length(),
          words("resolution", "resolution"),
        ]
      : [
          length(),
          words("shape", "aspect_ratio"),
          words("resolution", "resolution"),
          size(),
          typeof sound === "boolean" ? STUDIO_CONFIRM.sound(sound) : null,
        ];
  return [
    ...shown,
    typeof count === "number" && count > 1 ? STUDIO_CARD.count(count) : null,
    seed === null ? null : STUDIO_CARD.seed(String(seed)),
    ...refs,
  ].filter((p): p is string => p !== null);
}
