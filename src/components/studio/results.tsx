import { useEffect, useRef, useState, type CSSProperties, type ReactNode, type Ref } from "react";
import { Film, ImageIcon, Play, Star, Trash2 } from "lucide-react";
import clsx from "clsx";
import type { ErrorInfo } from "../../lib/types";
import { STUDIO_DELETE, STUDIO_TILE, STUDIO_VIEWER } from "../../lib/words";
import { toErrorInfo } from "../../store/app";
import { visiblePath } from "../FolderChip";
import { CheckboxField } from "../ui/CheckboxField";
import { FOCUS_RING } from "../ui/styles";
import * as api from "./api";
import { ConfirmDialog } from "./ConfirmDialog";
import { RefThumb } from "./RefTray";
import type { MediaKind, Output, Run } from "./types";
import { clock } from "./Working";

// Studio's results (0.1.0 Task B9), shared by the run cards, the gallery's grid and the viewer:
// a result's `blob:` URL (`useOutputUrl`), its tile and the question before a delete. A tile
// shows the thumbnail when there is one; else the file itself (a GIF, an SVG, a picture the
// worker could not shrink), always through an `img` element, where an SVG's scripts never run;
// else, for a video, its length and its start frame. It reads its picture only once it comes
// within a screen of what is shown. Since 0.2.0 Task U9 (design 10.4) a card's tile is the
// picture's own shape, up to 256 px (`cardTileStyle`); a tile's actions (its star, the grid's
// Delete) show on hover and while focus is in it, each named for its result; and the grid's
// Select mode puts a box on each tile in their place.

/** Where results were saved: the path left to right, with every invisible or
 *  direction-changing character as its code (`visiblePath`), as Settings shows a folder. */
export function SavedTo(props: { path: string }) {
  return (
    <>
      {STUDIO_VIEWER.savedTo} <bdi dir="ltr">{visiblePath(props.path)}</bdi>
    </>
  );
}

/** A result and the run that made it. */
export interface Result {
  run: Run;
  output: Output;
}

/**
 * Result `outputId`'s bytes, or its thumbnail's (`thumb`), as a `blob:` URL of type `mime`
 * (a thumbnail is always a JPEG), let go when the component unmounts or the result changes;
 * `null` until read. `error` says why the bytes could not be read, such as a video too big to
 * play here (`studio_too_big_to_show`).
 */
export function useOutputUrl(
  outputId: string,
  thumb: boolean,
  mime?: string,
): { url: string | null; error: ErrorInfo | null } {
  const type = thumb ? "image/jpeg" : (mime ?? "");
  const key = `${outputId}|${thumb}|${type}`;
  const [shown, setShown] = useState<{ key: string; url: string | null; error: ErrorInfo | null } | null>(null);
  useEffect(() => {
    let alive = true;
    let url: string | null = null;
    Promise.resolve()
      .then(() => api.studioOutputBytes(outputId, thumb))
      .then((bytes) => {
        if (!alive) return;
        url = URL.createObjectURL(new Blob([bytes], { type }));
        setShown({ key, url, error: null });
      })
      .catch((e: unknown) => {
        if (alive) setShown({ key, url: null, error: toErrorInfo(e) });
      });
    return () => {
      alive = false;
      if (url) URL.revokeObjectURL(url);
    };
  }, [key, outputId, thumb, type]);
  return shown?.key === key ? { url: shown.url, error: shown.error } : { url: null, error: null };
}

/** A tile's stand-in while its picture is read, or when it has none to show. */
function Placeholder({ kind, ref }: { kind: MediaKind; ref?: Ref<HTMLSpanElement> }) {
  const Icon = kind === "video" ? Film : ImageIcon;
  return (
    <span ref={ref} className="flex size-full items-center justify-center text-fg-muted">
      <Icon size={18} aria-hidden />
    </span>
  );
}

/** The nearest ancestor of `el` that scrolls up and down; `null` for the window. */
function scrollParent(el: HTMLElement): HTMLElement | null {
  for (let at = el.parentElement; at; at = at.parentElement) {
    const { overflowY } = getComputedStyle(at);
    if (overflowY === "auto" || overflowY === "scroll") return at;
  }
  return null;
}

/** `children` once the tile comes within one screen of what is shown (its picture is read
 *  only then, so a page of tiles does not read every picture at once); its stand-in until
 *  then. Where the webview cannot tell (no IntersectionObserver), at once. */
function WhenNear(props: { kind: MediaKind; children: ReactNode }) {
  const [near, setNear] = useState(() => typeof IntersectionObserver === "undefined");
  const spot = useRef<HTMLSpanElement>(null);
  useEffect(() => {
    const el = spot.current;
    if (near || !el) return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (!entries.some((e) => e.isIntersecting)) return;
        observer.disconnect();
        setNear(true);
      },
      // A screen's height ahead, above and below, of the list that scrolls.
      { root: scrollParent(el), rootMargin: "100% 0px" },
    );
    observer.observe(el);
    return () => observer.disconnect();
  }, [near]);
  return near ? props.children : <Placeholder kind={props.kind} ref={spot} />;
}

/** A tile's picture: the thumbnail, else the file itself, always in an `img` element. */
function TilePicture(props: { output: Output; kind: MediaKind }) {
  const { output } = props;
  const { url } = useOutputUrl(output.id, output.hasThumb, output.mime);
  return url ? <img src={url} alt="" className="size-full object-cover" /> : <Placeholder kind={props.kind} />;
}

/** The results of `run` in their order. */
export function resultsOf(run: Run): Output[] {
  return [...run.outputs].sort((a, b) => a.idx - b.idx);
}

/** The longest side of a card's tile (design 10.4). */
const CARD_TILE_PX = 256;

/** `text` as two sides, "16:9" or "832x480"; `null` when it is not that. */
function sidesOf(text: unknown): [number, number] | null {
  if (typeof text !== "string") return null;
  const [w, h] = text.split(/[:x×]/).map(Number);
  return w !== undefined && h !== undefined && w > 0 && h > 0 ? [w, h] : null;
}

/** A card's tile at its picture's own shape, its longer side 256 px: the result's size, else the
 *  run's exact size or shape, else square. Never wider than the card (`max-w-full` on the tile). */
export function cardTileStyle(run: Run, output: Output): CSSProperties {
  const [w, h] =
    output.width && output.height && output.width > 0 && output.height > 0
      ? [output.width, output.height]
      : (sidesOf(run.params.size) ?? sidesOf(run.params.aspect_ratio) ?? [1, 1]);
  return w >= h
    ? { width: CARD_TILE_PX, aspectRatio: `${w} / ${h}` }
    : { height: CARD_TILE_PX, aspectRatio: `${w} / ${h}` };
}

/** A tile's action over its picture: shown on hover and while focus is in the tile (so the
 *  keyboard reaches it), or always (`shown`: a starred result's star). */
function tileAction(shown: boolean): string {
  return clsx(
    "grid size-7 place-items-center rounded-lg bg-black/55 text-white backdrop-blur-sm transition-opacity hover:bg-black/75",
    !shown && "opacity-0 group-hover:opacity-100 group-focus-within:opacity-100",
    FOCUS_RING,
  );
}

/** One result's tile: it opens the viewer. Its star (`onStar`) and Delete (`onDelete`, the
 *  grid's) show over it on hover and focus, its star always while starred. In the grid's Select
 *  mode (`select`) it has a box instead, and a click on the picture ticks it. `caption` shows the
 *  prompt along its foot on hover (the grid's). */
export function ResultTile(props: {
  run: Run;
  output: Output;
  onOpen(outputId: string): void;
  /** The box of the grid's Select mode, and whether the result is selected. */
  select?: { selected: boolean; onChange(selected: boolean): void };
  /** Its star, pressed while the result is starred. */
  onStar?: () => void;
  /** Its Delete (which asks first). */
  onDelete?: () => void;
  caption?: boolean;
  className?: string;
  style?: CSSProperties;
}) {
  const { run, output, select } = props;
  const all = resultsOf(run);
  const n = all.findIndex((o) => o.id === output.id) + 1;
  const of = all.length;
  const startFrame = run.refs.find((r) => r.role === "startFrame") ?? null;
  const media =
    run.kind === "image" || output.hasThumb ? (
      <TilePicture output={output} kind={run.kind} />
    ) : startFrame ? (
      // A video whose poster frame is not saved yet: its start frame, when it had one.
      <RefThumb sha256={startFrame.sha256} mime={startFrame.mime} alt="" className="size-full" />
    ) : (
      <Placeholder kind="video" />
    );
  const selected = select?.selected ?? false;
  return (
    <div
      data-output-tile={output.id}
      data-selected={selected || undefined}
      style={props.style}
      className={clsx(
        "group relative shrink-0 overflow-hidden rounded-xl bg-bg-muted ring-accent transition-shadow",
        selected && "ring-[3px]",
        props.className,
      )}
    >
      {select ? (
        // The box below is what a keyboard or screen reader uses; the picture is a larger target
        // for the pointer.
        <div
          data-tile-picture
          aria-hidden
          onClick={() => select.onChange(!selected)}
          className={clsx("size-full cursor-pointer transition-transform duration-150", selected && "scale-[0.92]")}
        >
          <WhenNear kind={run.kind}>{media}</WhenNear>
        </div>
      ) : (
        <button
          type="button"
          aria-label={STUDIO_TILE.open(run.kind, n, of, run.prompt)}
          onClick={() => props.onOpen(output.id)}
          className="block size-full outline-none focus-visible:ring-[3px] focus-visible:ring-accent focus-visible:ring-inset"
        >
          <WhenNear kind={run.kind}>{media}</WhenNear>
        </button>
      )}
      {props.caption && !select && run.prompt && (
        <p
          aria-hidden
          className="pointer-events-none absolute inset-x-0 bottom-0 line-clamp-2 bg-linear-to-t from-black/75 to-transparent px-2.5 pt-6 pb-2 text-sm text-white opacity-0 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100"
        >
          {run.prompt}
        </p>
      )}
      {run.kind === "video" && output.durationMs !== null && (
        <span className="pointer-events-none absolute right-2 bottom-2 inline-flex items-center gap-1 rounded-md bg-black/60 px-1.5 py-px text-sm font-medium text-white tabular-nums backdrop-blur-sm">
          <Play size={11} aria-hidden fill="currentColor" />
          {clock(output.durationMs)}
        </span>
      )}
      {select ? (
        <div className="absolute top-2 left-2">
          <CheckboxField
            label={STUDIO_TILE.select(run.kind, n, of, run.prompt)}
            hideLabel
            tone="overlay"
            checked={selected}
            onChange={select.onChange}
          />
        </div>
      ) : (
        (props.onStar || props.onDelete) && (
          <div data-tile-actions className="absolute top-1.5 right-1.5 flex gap-1">
            {props.onStar && (
              <button
                type="button"
                aria-label={STUDIO_TILE.star(run.kind, n, of)}
                aria-pressed={output.starred}
                onClick={props.onStar}
                className={tileAction(output.starred)}
              >
                <Star size={14} aria-hidden fill={output.starred ? "currentColor" : "none"} />
              </button>
            )}
            {props.onDelete && (
              <button
                type="button"
                aria-label={STUDIO_TILE.delete(run.kind, n, of)}
                onClick={props.onDelete}
                className={tileAction(false)}
              >
                <Trash2 size={14} aria-hidden />
              </button>
            )}
          </div>
        )
      )}
      {!select && !props.onStar && output.starred && (
        <Star
          size={14}
          aria-hidden
          fill="currentColor"
          className="pointer-events-none absolute top-2 right-2 text-white drop-shadow"
        />
      )}
    </div>
  );
}

/** When the local day `daysBack` days before today started, in Unix ms (today's: 0): the day
 *  the daily limit counts by, and the one a deleted cloud run still counts toward. */
export function localDayStart(daysBack = 0): number {
  const day = new Date();
  day.setHours(0, 0, 0, 0);
  day.setDate(day.getDate() - daysBack);
  return day.getTime();
}

/** The question before results are deleted: what goes, what the audit log keeps and, when a
 *  cloud run made today is among theirs, that it still counts toward today's spending (a run
 *  made on an earlier day no longer counts toward the daily limit). */
export function ConfirmDelete(props: {
  /** The results to delete; `null` while the question is not asked. */
  results: Result[] | null;
  onConfirm(): void;
  onCancel(): void;
}) {
  // A closed question stays on screen while it fades out (styles.css, 130 ms): it keeps the
  // words it asked with, not an empty title and body.
  const [asked, setAsked] = useState(props.results);
  if (props.results !== null && props.results !== asked) setAsked(props.results);
  const results = props.results ?? asked;
  const today = localDayStart();
  const counts = results?.some((r) => r.run.runsOn === "cloud" && r.run.createdAt >= today) ?? false;
  return (
    <ConfirmDialog
      open={props.results !== null}
      onOpenChange={(open) => {
        if (!open) props.onCancel();
      }}
      title={
        results &&
        STUDIO_DELETE.title(
          results.length,
          results.map((r) => r.run.kind),
        )
      }
      body={results && STUDIO_DELETE.removes(results.length)}
      note={
        <>
          {STUDIO_DELETE.audit}
          {counts ? ` ${STUDIO_DELETE.spending}` : ""}
        </>
      }
      cancelLabel={STUDIO_DELETE.cancel}
      actionLabel={STUDIO_DELETE.delete}
      destructive
      onConfirm={props.onConfirm}
    />
  );
}
