import { useMemo, useState, type KeyboardEvent } from "react";
import { CheckSquare, Images, Trash2 } from "lucide-react";
import type { ErrorInfo } from "../../lib/types";
import { STUDIO_GRID, STUDIO_RESULTS, STUDIO_TABS } from "../../lib/words";
import { toErrorInfo } from "../../store/app";
import { useStudio } from "../../store/studio";
import { ErrorBanner } from "../ErrorBanner";
import { ChipGroup } from "../ui/ChipGroup";
import { SelectField } from "../ui/SelectField";
import { FOCUS_RING } from "../ui/styles";
import { ConfirmDelete, localDayStart, ResultTile, resultsOf, SavedTo, type Result } from "./results";
import { CARD_BUTTON, GHOST_BUTTON, PANEL_BUTTON } from "./styles";
import type { MediaKind, Run, RunsFilter } from "./types";

// Studio's gallery grid (0.1.0 Task B9; studio-design 5): every result of every kind as a tile
// (./results), newest run first, in place of the card stream while Grid is on. Beside the top
// bar's search and Starred, the grid filters by kind (All, Pictures, Video; the top bar's
// Pictures and Video are hidden meanwhile) and date; *Load more* reads the next page. Since
// 0.2.0 Task U9 (design 10.4) its tiles are about 220 px, each with its star and Delete on
// hover and focus and its prompt along its foot; there is no box on every tile: *Select* turns
// on a mode with a box on each and a bar of actions on the selection, which deletes them (after
// the question that says what the audit log keeps) or saves them into one folder together, and
// says where. *Done*, Esc or a finished action ends the mode.

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

/** The grid's kinds, as its chips name them ("all": no kind). */
type Kind = MediaKind | "all";

/** *Select* while its mode is on (it reads *Done*): in the accent. */
const SELECTING =
  "inline-flex items-center gap-1.5 rounded-lg border border-accent/50 bg-accent/10 px-2.5 py-1.5 text-sm font-medium text-accent " +
  FOCUS_RING;

/** The gallery's grid; `onOpen` opens a result in the viewer. */
export function Grid(props: { onOpen(outputId: string): void }) {
  const runs = useStudio((s) => s.runs);
  const live = useStudio((s) => s.live);
  const filter = useStudio((s) => s.filter);
  const next = useStudio((s) => s.next);
  const results = useMemo(() => galleryResults(runs, live, filter), [runs, live, filter]);
  const when = whenOf(filter.since);
  // Select mode, and the results ticked in it.
  const [selecting, setSelecting] = useState(false);
  const [selected, setSelected] = useState<ReadonlySet<string>>(() => new Set());
  // The results the delete question asks about: the selection, or one tile's own Delete.
  const [asking, setAsking] = useState<Result[] | null>(null);
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
  // Select mode ends with its selection.
  const endSelect = () => {
    setSelecting(false);
    setSelected(new Set());
  };
  const kind: Kind = filter.kind ?? "all";
  const filtered = Boolean(filter.kind || filter.starred || filter.text?.trim() || filter.since != null);
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    // Esc ends Select mode; not an Esc meant for a menu or question opened from here (drawn
    // elsewhere, its keys reach here through React all the same).
    if (e.key !== "Escape" || !selecting || e.defaultPrevented) return;
    if (!(e.target instanceof Node) || !e.currentTarget.contains(e.target)) return;
    e.preventDefault();
    endSelect();
  };

  return (
    <div data-grid className="flex flex-col gap-4" onKeyDown={onKeyDown}>
      <div className="flex flex-wrap items-center gap-2">
        <ChipGroup<Kind>
          label={STUDIO_GRID.kinds}
          value={kind}
          size="md"
          options={(["all", "image", "video"] as const).map((k) => ({
            value: k,
            label: k === "all" ? STUDIO_GRID.all : STUDIO_TABS[k],
          }))}
          onChange={(k) => perform(() => store().setFilter({ kind: k === "all" ? null : k }))}
        />
        <div className="w-36">
          <SelectField<When>
            label={STUDIO_GRID.when}
            hideLabel
            value={when}
            options={WHEN.map((w) => ({ value: w, label: STUDIO_GRID.since[w] }))}
            onChange={(w) => perform(() => store().setFilter({ since: sinceOf(w) }))}
          />
        </div>
        {results.length > 0 && (
          <button
            type="button"
            onClick={() => (selecting ? endSelect() : setSelecting(true))}
            className={`${selecting ? SELECTING : PANEL_BUTTON} ml-auto`}
          >
            <CheckSquare size={15} aria-hidden />
            {selecting ? STUDIO_RESULTS.done : STUDIO_RESULTS.select}
          </button>
        )}
      </div>
      {selecting && (
        <div
          role="toolbar"
          aria-label={STUDIO_RESULTS.selection}
          className="enter sticky top-2 z-10 flex flex-wrap items-center gap-2 rounded-xl border border-border bg-surface px-3 py-2 shadow-float"
        >
          <span className="mr-auto text-sm font-medium tabular-nums">{STUDIO_RESULTS.selected(chosen.length)}</span>
          {chosen.length > 0 && (
            <button type="button" className={GHOST_BUTTON} onClick={() => setSelected(new Set())}>
              {STUDIO_GRID.clear}
            </button>
          )}
          <button
            type="button"
            className={`${PANEL_BUTTON} text-danger`}
            disabled={busy || chosen.length === 0}
            onClick={() => setAsking(chosen)}
          >
            <Trash2 size={15} aria-hidden />
            {STUDIO_RESULTS.deleteSelected(chosen.length)}
          </button>
          <button
            type="button"
            className={PANEL_BUTTON}
            disabled={busy || chosen.length === 0}
            onClick={() =>
              perform(async () => {
                const folder = await store().saveSeveral(ids);
                // A cancelled folder picker leaves the selection as it was.
                if (folder === null) return;
                endSelect();
                setSaved(folder);
              })
            }
          >
            {STUDIO_GRID.saveSome(chosen.length)}
          </button>
        </div>
      )}
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
      {saved && (
        <p role="status" className="text-sm text-fg-muted">
          <SavedTo path={saved} />
        </p>
      )}
      {results.length === 0 ? (
        <div className="flex flex-col items-center gap-3 py-16 text-center text-fg-muted">
          <span className="grid size-12 place-items-center rounded-2xl bg-bg-muted">
            <Images size={22} aria-hidden />
          </span>
          <p className="text-sm">{filtered ? STUDIO_GRID.nothingMatches : STUDIO_GRID.empty}</p>
        </div>
      ) : (
        <div className="grid grid-cols-[repeat(auto-fill,minmax(12rem,1fr))] gap-3">
          {results.map((result) => {
            const { run, output } = result;
            return (
              <ResultTile
                key={output.id}
                run={run}
                output={output}
                onOpen={props.onOpen}
                caption
                select={selecting ? { selected: selected.has(output.id), onChange: (on) => choose(output.id, on) } : undefined}
                onStar={() => perform(() => store().star(output.id, !output.starred))}
                onDelete={() => setAsking([result])}
                className="aspect-square shadow-soft"
              />
            );
          })}
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
        results={asking}
        onCancel={() => setAsking(null)}
        onConfirm={() => {
          const gone = (asking ?? []).map((r) => r.output.id);
          setAsking(null);
          perform(() => store().remove(gone), endSelect);
        }}
      />
    </div>
  );
}
