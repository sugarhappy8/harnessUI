import { useEffect, useRef, useState, type ReactNode, type Ref } from "react";
import { Film, ImageIcon, Star } from "lucide-react";
import type { ErrorInfo } from "../../lib/types";
import { STUDIO_DELETE, STUDIO_TILE } from "../../lib/words";
import { toErrorInfo } from "../../store/app";
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
// within a screen of what is shown.

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

/** One result's tile: it opens the viewer. In the grid it has a box to select it with others;
 *  on a card, a star; elsewhere a starred result shows its star. */
export function ResultTile(props: {
  run: Run;
  output: Output;
  onOpen(outputId: string): void;
  /** The grid's box, and whether the result is selected. */
  select?: { selected: boolean; onChange(selected: boolean): void };
  /** The card's star, pressed while the result is starred. */
  onStar?: () => void;
  className: string;
}) {
  const { run, output } = props;
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
  return (
    <div
      data-output-tile={output.id}
      className={`relative overflow-hidden rounded-lg border border-border bg-bg-muted ${props.className}`}
    >
      <button
        type="button"
        aria-label={STUDIO_TILE.open(run.kind, n, of, run.prompt)}
        onClick={() => props.onOpen(output.id)}
        className="block size-full"
      >
        <WhenNear kind={run.kind}>{media}</WhenNear>
      </button>
      {run.kind === "video" && output.durationMs !== null && (
        <span className="pointer-events-none absolute right-1 bottom-1 rounded bg-black/60 px-1 text-xs text-white tabular-nums">
          {clock(output.durationMs)}
        </span>
      )}
      {props.select && (
        <input
          type="checkbox"
          aria-label={STUDIO_TILE.select(run.kind, n, of, run.prompt)}
          checked={props.select.selected}
          onChange={(e) => props.select?.onChange(e.target.checked)}
          className="absolute top-1.5 left-1.5 size-4"
        />
      )}
      {props.onStar ? (
        <button
          type="button"
          aria-label={STUDIO_TILE.star(run.kind, n, of)}
          aria-pressed={output.starred}
          onClick={props.onStar}
          className="absolute top-1 right-1 rounded-md bg-black/50 p-1 text-white hover:bg-black/70"
        >
          <Star size={12} aria-hidden fill={output.starred ? "currentColor" : "none"} />
        </button>
      ) : (
        output.starred && (
          <Star
            size={14}
            aria-hidden
            fill="currentColor"
            className="pointer-events-none absolute top-1.5 right-1.5 text-white drop-shadow"
          />
        )
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
  const { results } = props;
  const today = localDayStart();
  const counts = results?.some((r) => r.run.runsOn === "cloud" && r.run.createdAt >= today) ?? false;
  return (
    <ConfirmDialog
      open={results !== null}
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
