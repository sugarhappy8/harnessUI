import type { MemoryBar, OffloadPreview } from "../lib/types";
import {
  amountGb,
  appleLimitWords,
  barLegend,
  barTitle,
  legendWords,
  needsWords,
  pictureCaption,
  pictureLayerWords,
  pictureSourceWords,
} from "../lib/words";

/** The colour of each part of a bar; every part also has its word and number in the legend. */
const PART_CLASS: Record<ReturnType<typeof barLegend>[number]["part"], string> = {
  model: "bg-accent",
  context: "bg-accent-2",
  compute: "bg-accent/45",
  imageReader: "bg-success/70",
  other: "bg-fg-muted/35",
  free: "bg-transparent",
};

/**
 * One memory bar: stacked parts in proportion to the bar's memory (or to the parts' sum
 * when the model needs more than there is), its legend in words and numbers, and what the
 * model needs of the memory that is free.
 */
function Bar(props: { bar: MemoryBar; kind: "graphics" | "computer" | "shared"; limitBytes?: number | null }) {
  const { bar, kind, limitBytes } = props;
  const title = barTitle(bar, kind);
  const legend = barLegend(bar);
  const whole = Math.max(
    bar.totalBytes > 0 ? bar.totalBytes : 0,
    legend.reduce((a, i) => a + (i.bytes > 0 ? i.bytes : 0), 0),
    1,
  );
  const share = (bytes: number) => `${Math.min(100, Math.max(0, (bytes / whole) * 100)).toFixed(2)}%`;
  const sentence = `${title}: ${legend.map((i) => `${i.words.toLowerCase()} ${amountGb(i.bytes)}`).join(", ")}.`;
  const marker = kind === "shared" && limitBytes != null && limitBytes > 0 && bar.totalBytes > 0 ? limitBytes : null;
  return (
    <div className="space-y-1" data-bar={kind}>
      <p className="text-xs font-medium">{title}</p>
      <div className="relative h-3 w-full overflow-hidden rounded-full bg-bg-muted" role="img" aria-label={sentence}>
        <div className="flex h-full w-full">
          {legend
            .filter((i) => i.part !== "free" && i.bytes > 0)
            .map((i) => (
              <div key={i.part} className={`h-full ${PART_CLASS[i.part]}`} style={{ width: share(i.bytes) }} />
            ))}
        </div>
        {marker !== null && (
          <div
            className="absolute top-0 h-full w-0.5 bg-fg"
            style={{ left: share(marker) }}
            data-marker="apple-limit"
            aria-hidden
          />
        )}
      </div>
      <ul className="flex flex-wrap gap-x-3 gap-y-0.5 text-[11px] text-fg-muted tabular-nums">
        {legend.map((i) => (
          <li key={i.part} className="flex items-center gap-1">
            <span
              className={`inline-block h-2 w-2 rounded-sm ${i.part === "free" ? "border border-border" : PART_CLASS[i.part]}`}
              aria-hidden
            />
            {legendWords(i)}
          </li>
        ))}
      </ul>
      {marker !== null && <p className="text-[11px] text-fg-muted">{appleLimitWords(marker)}</p>}
      <p className="text-xs text-fg-muted">{needsWords(bar)}</p>
    </div>
  );
}

/**
 * "Where the model goes" (item 6, 6.2): the memory each part of the model would take on the
 * graphics card and in computer memory with the choices on screen, the layers on each, and
 * where the figures come from. It follows every change at once (the instant estimate);
 * llama.cpp's own measurement replaces it while Fine control is open. Warnings are not
 * here: the panel shows one warning line at the top (4.1).
 */
export function OffloadPicture(props: {
  picture: OffloadPreview;
  /** Exact figures are being worked out for the choices on screen. */
  measuring: boolean;
  fineOpen: boolean;
  /** Apple graphics (the help and layer lines say so). */
  apple: boolean;
  /** A Mac (the caption names Activity Monitor). */
  mac: boolean;
}) {
  const { picture, measuring, fineOpen, apple, mac } = props;
  const layers = pictureLayerWords(picture, apple);
  return (
    <section className="space-y-3 rounded-md border border-border px-3 py-2" aria-labelledby="offload-picture-title">
      <h3 id="offload-picture-title" className="text-sm font-medium">
        Where the model goes
      </h3>
      {picture.unified ? (
        <Bar bar={picture.computer} kind="shared" limitBytes={picture.graphicsLimitBytes} />
      ) : (
        <>
          {picture.graphics && <Bar bar={picture.graphics} kind="graphics" />}
          <Bar bar={picture.computer} kind="computer" />
        </>
      )}
      <div className="space-y-1 text-xs">
        {layers && <p data-picture="layers">{layers}</p>}
        <p className="text-fg-muted" data-picture="source">
          {pictureSourceWords(picture.source, { measuring, fineOpen })}
        </p>
        <p className="text-fg-muted">{pictureCaption(mac)}</p>
      </div>
    </section>
  );
}
