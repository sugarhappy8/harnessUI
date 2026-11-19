import { useMemo, useState } from "react";
import type { ErrorInfo } from "../../lib/types";
import { STUDIO_GRID, STUDIO_TABS } from "../../lib/words";
import { toErrorInfo } from "../../store/app";
import { useStudio } from "../../store/studio";
import { ErrorBanner } from "../ErrorBanner";
import { ConfirmDelete, localDayStart, ResultTile, resultsOf, SavedTo, type Result } from "./results";
import { CARD_BUTTON } from "./styles";
import type { Run, RunsFilter } from "./types";

// Studio's gallery grid (0.1.0 Task B9; studio-design 5): every result of every kind as a tile
// (./results), newest run first, in place of the card stream while Grid is on. Beside the top
// bar's search and Starred, the grid filters by kind (All, Pictures, Video; the top bar's
// Pictures and Video are hidden meanwhile) and date; *Load more* reads the next page. Several
// results can be selected and deleted (after the question that says what the audit log keeps)
// or saved into one folder together, which says where.

/** The date filter's choices: from the start of the local day `DAYS` days back, today counted. */
type When = "any" | "today" | "week" | "month";
const WHEN: readonly When[] = ["any", "today", "week", "month"];
const DAYS: Record<Exclude<When, "any">, number> = { today: 1, week: 7, month: 30 };

/** The first moment `when` covers: the start of the local day `DAYS[when] - 1` days before today. */
function sinceOf(when: When): number | null {
  return when === "any" ? null : localDayStart(DAYS[when] - 1);
}

/** The date choice the gallery's filter has (`since`): the one whose first moment it is, or
 *  the nearest once a day has passed since it was chosen; Any time without one. */
function whenOf(since: number | null | undefined): When {
  if (since == null) return "any";
  const gap = (when: When) => Math.abs((sinceOf(when) ?? Number.NEGATIVE_INFINITY) - since);
  return WHEN.reduce<When>((nearest, when) => (gap(when) < gap(nearest) ? when : nearest), "today");
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
  const when = whenOf(filter.since);
  const [selected, setSelected] = useState<ReadonlySet<string>>(() => new Set());
  const [asking, setAsking] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  // Where several results were saved, until the next action.
  const [saved, setSaved] = useState<string | null>(null);
  // A result selected and since gone (deleted, or filtered out) does not count.
  const chosen = results.filter((r) => selected.has(r.output.id));
  const ids = chosen.map((r) => r.output.id);

  const perform = (action: () => Promise<void>, after?: () => void) => {
    setBusy(true);
    setError(null);
    setSaved(null);
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
          onChange={(e) => perform(() => store().setFilter({ since: sinceOf(e.target.value as When) }))}
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
              onClick={() =>
                perform(async () => {
                  const folder = await store().saveSeveral(ids);
                  // A cancelled folder picker leaves the selection as it was.
                  if (folder === null) return;
                  clear();
                  setSaved(folder);
                })
              }
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
      {saved && (
        <p role="status" className="text-sm text-fg-muted">
          <SavedTo path={saved} />
        </p>
      )}
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
