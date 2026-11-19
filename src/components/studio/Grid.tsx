import { useEffect, useMemo, useState } from "react";
import { AlertDialog } from "radix-ui";
import { Film, ImageIcon, Star } from "lucide-react";
import type { ErrorInfo } from "../../lib/types";
import { STUDIO_DELETE, STUDIO_GRID, STUDIO_TABS, STUDIO_TILE } from "../../lib/words";
import { toErrorInfo } from "../../store/app";
import { useStudio } from "../../store/studio";
import { ErrorBanner } from "../ErrorBanner";
import * as api from "./api";
import { RefThumb } from "./RefTray";
import { CARD_BUTTON } from "./styles";
import type { MediaKind, Output, Run, RunsFilter } from "./types";
import { clock } from "./Working";

// Studio's gallery grid (0.1.0 Task B9; studio-design 5): every result of every kind as a tile,
// newest run first, in place of the card stream while Grid is on. Beside the top bar's search
// and Starred, the grid filters by kind (All, Pictures, Video) and date; *Load more* reads the
// next page. Several results can be selected and deleted (after the question that says what
// the audit log keeps) or saved into one folder together. A tile shows the thumbnail when
// there is one; else the file itself (a GIF, an SVG, a picture the worker could not shrink),
// always through an `img` element, where an SVG's scripts never run; else, for a video, its
// length and its start frame. Also here, shared with the run cards and the viewer: a result's
// `blob:` URL (`useOutputUrl`), its tile, and the question before a delete.

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
function Placeholder(props: { kind: MediaKind }) {
  const Icon = props.kind === "video" ? Film : ImageIcon;
  return (
    <span className="flex size-full items-center justify-center text-fg-muted">
      <Icon size={18} aria-hidden />
    </span>
  );
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
        {media}
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

/** The question before results are deleted: what goes, what the audit log keeps and, when a
 *  cloud run's result is among them, that it still counts toward today's spending. */
export function ConfirmDelete(props: {
  /** The results to delete; `null` while the question is not asked. */
  results: Result[] | null;
  onConfirm(): void;
  onCancel(): void;
}) {
  const { results } = props;
  const cloud = results?.some((r) => r.run.runsOn === "cloud") ?? false;
  return (
    <AlertDialog.Root
      open={results !== null}
      onOpenChange={(open) => {
        if (!open) props.onCancel();
      }}
    >
      <AlertDialog.Portal>
        <AlertDialog.Overlay className="dialog-overlay fixed inset-0" />
        <AlertDialog.Content className="fixed top-1/2 left-1/2 w-[26rem] max-w-[92vw] -translate-x-1/2 -translate-y-1/2 rounded-2xl border border-border bg-surface p-5 text-sm shadow-pop">
          {results && (
            <>
              <AlertDialog.Title className="text-base font-semibold">
                {STUDIO_DELETE.title(
                  results.length,
                  results.map((r) => r.run.kind),
                )}
              </AlertDialog.Title>
              <AlertDialog.Description asChild>
                <div className="mt-2 flex flex-col gap-1.5">
                  <p>{STUDIO_DELETE.removes(results.length)}</p>
                  <p className="text-fg-muted">
                    {STUDIO_DELETE.audit}
                    {cloud ? ` ${STUDIO_DELETE.spending}` : ""}
                  </p>
                </div>
              </AlertDialog.Description>
              <div className="mt-4 flex justify-end gap-2">
                <AlertDialog.Cancel className="rounded-md border border-border px-3 py-1.5 hover:bg-bg-muted">
                  {STUDIO_DELETE.cancel}
                </AlertDialog.Cancel>
                <AlertDialog.Action
                  onClick={props.onConfirm}
                  className="rounded-md bg-danger px-3 py-1.5 font-medium text-white hover:opacity-90"
                >
                  {STUDIO_DELETE.delete}
                </AlertDialog.Action>
              </div>
            </>
          )}
        </AlertDialog.Content>
      </AlertDialog.Portal>
    </AlertDialog.Root>
  );
}

/** The date filter's choices: from the start of the local day `DAYS` days back, today counted. */
type When = "any" | "today" | "week" | "month";
const WHEN: readonly When[] = ["any", "today", "week", "month"];
const DAYS: Record<Exclude<When, "any">, number> = { today: 1, week: 7, month: 30 };

/** The first moment `when` covers: the start of the local day `DAYS[when] - 1` days before today. */
function sinceOf(when: When): number | null {
  if (when === "any") return null;
  const day = new Date();
  day.setHours(0, 0, 0, 0);
  day.setDate(day.getDate() - (DAYS[when] - 1));
  return day.getTime();
}

/** Whether a run reported since the gallery was read belongs in it: of the kind shown, made
 *  since the date chosen, its prompt holding the search. */
function matches(run: Run, filter: RunsFilter): boolean {
  const text = filter.text?.trim().toLowerCase() ?? "";
  return (
    (!filter.kind || run.kind === filter.kind) &&
    (!filter.model || run.model === filter.model) &&
    (filter.since == null || run.createdAt >= filter.since) &&
    (filter.until == null || run.createdAt < filter.until) &&
    (text === "" || run.prompt.toLowerCase().includes(text))
  );
}

/** The gallery's results, newest run first, each run's in their order: the runs read, at their
 *  newest state, and the runs reported since that the filter lets through; with Starred on,
 *  only the starred results. */
function galleryResults(runs: Run[], live: Record<string, Run>, filter: RunsFilter): Result[] {
  const byId = new Map<string, Run>();
  for (const run of runs) byId.set(run.id, live[run.id] ?? run);
  for (const run of Object.values(live)) if (!byId.has(run.id) && matches(run, filter)) byId.set(run.id, run);
  return [...byId.values()]
    .sort((a, b) => b.createdAt - a.createdAt)
    .flatMap((run) =>
      resultsOf(run)
        .filter((output) => !filter.starred || output.starred)
        .map((output) => ({ run, output })),
    );
}

const TOGGLE =
  "rounded-md px-2.5 py-1 text-sm aria-pressed:bg-surface aria-pressed:font-medium aria-pressed:shadow-soft hover:text-fg";

/** The gallery's grid; `onOpen` opens a result in the viewer. */
export function Grid(props: { onOpen(outputId: string): void }) {
  const runs = useStudio((s) => s.runs);
  const live = useStudio((s) => s.live);
  const filter = useStudio((s) => s.filter);
  const next = useStudio((s) => s.next);
  const results = useMemo(() => galleryResults(runs, live, filter), [runs, live, filter]);
  const [when, setWhen] = useState<When>("any");
  const [selected, setSelected] = useState<ReadonlySet<string>>(() => new Set());
  const [asking, setAsking] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  // A result selected and since gone (deleted, or filtered out) does not count.
  const chosen = results.filter((r) => selected.has(r.output.id));
  const ids = chosen.map((r) => r.output.id);

  const perform = (action: () => Promise<void>, after?: () => void) => {
    setBusy(true);
    setError(null);
    action().then(
      () => {
        setBusy(false);
        after?.();
      },
      (e: unknown) => {
        setBusy(false);
        setError(toErrorInfo(e));
      },
    );
  };
  const store = useStudio.getState;
  const choose = (id: string, on: boolean) =>
    setSelected((now) => {
      const changed = new Set(now);
      if (on) changed.add(id);
      else changed.delete(id);
      return changed;
    });
  const clear = () => setSelected(new Set());
  const kind = filter.kind ?? null;
  const filtered = Boolean(kind || filter.starred || filter.text?.trim() || filter.since != null);

  return (
    <div data-grid className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <div role="group" aria-label={STUDIO_GRID.kinds} className="flex gap-0.5 rounded-lg bg-bg-muted p-0.5 text-fg-muted">
          {([null, "image", "video"] as const).map((k) => (
            <button
              key={k ?? "all"}
              type="button"
              aria-pressed={kind === k}
              onClick={() => perform(() => store().setFilter({ kind: k }))}
              className={TOGGLE}
            >
              {k ? STUDIO_TABS[k] : STUDIO_GRID.all}
            </button>
          ))}
        </div>
        <select
          aria-label={STUDIO_GRID.when}
          value={when}
          onChange={(e) => {
            const chosenWhen = e.target.value as When;
            setWhen(chosenWhen);
            perform(() => store().setFilter({ since: sinceOf(chosenWhen) }));
          }}
          className="rounded-lg border border-border bg-bg px-2 py-1 text-sm"
        >
          {WHEN.map((w) => (
            <option key={w} value={w}>
              {STUDIO_GRID.since[w]}
            </option>
          ))}
        </select>
        {chosen.length > 0 && (
          <div className="ml-auto flex flex-wrap items-center gap-2">
            <span className="text-xs text-fg-muted">{STUDIO_GRID.selected(chosen.length)}</span>
            <button type="button" className={CARD_BUTTON} disabled={busy} onClick={() => setAsking(true)}>
              {STUDIO_GRID.deleteSome(chosen.length)}
            </button>
            <button
              type="button"
              className={CARD_BUTTON}
              disabled={busy}
              onClick={() => perform(() => store().saveSeveral(ids), clear)}
            >
              {STUDIO_GRID.saveSome(chosen.length)}
            </button>
            <button type="button" className="text-xs text-fg-muted underline hover:text-fg" onClick={clear}>
              {STUDIO_GRID.clear}
            </button>
          </div>
        )}
      </div>
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
      {results.length === 0 ? (
        <p className="py-8 text-center text-sm text-fg-muted">{filtered ? STUDIO_GRID.nothingMatches : STUDIO_GRID.empty}</p>
      ) : (
        <div className="grid grid-cols-[repeat(auto-fill,minmax(9rem,1fr))] gap-2">
          {results.map(({ run, output }) => (
            <ResultTile
              key={output.id}
              run={run}
              output={output}
              onOpen={props.onOpen}
              select={{ selected: selected.has(output.id), onChange: (on) => choose(output.id, on) }}
              className="aspect-square"
            />
          ))}
        </div>
      )}
      {next !== null && (
        <button
          type="button"
          className={`${CARD_BUTTON} self-center`}
          disabled={busy}
          onClick={() => perform(() => store().loadMore())}
        >
          {STUDIO_GRID.loadMore}
        </button>
      )}
      <ConfirmDelete
        results={asking ? chosen : null}
        onCancel={() => setAsking(false)}
        onConfirm={() => {
          setAsking(false);
          perform(() => store().remove(ids), clear);
        }}
      />
    </div>
  );
}
