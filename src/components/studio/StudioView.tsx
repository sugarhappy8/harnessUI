import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { Tabs } from "radix-ui";
import * as ipc from "../../lib/ipc";
import type { ErrorInfo } from "../../lib/types";
import { STUDIO_GRID } from "../../lib/words";
import { toErrorInfo } from "../../store/app";
import { useStudio, type StudioTab } from "../../store/studio";
import { ErrorBanner } from "../ErrorBanner";
import { Notices } from "./EmptyStates";
import { Grid } from "./Grid";
import { LocalModels } from "./LocalModels";
import { PromptDock } from "./PromptDock";
import { useNativePictureDrops } from "./RefTray";
import { queueSummary, RunCard } from "./RunCard";
import { SettingsPanel } from "./SettingsPanel";
import { CARD_BUTTON } from "./styles";
import { TopBar } from "./TopBar";
import type { Run } from "./types";
import { Viewer } from "./Viewer";

// Studio's view (0.1.0, design Wireframe 1): the top bar (TopBar.tsx), the card stream in the
// middle with the notices and empty states above it, local first (EmptyStates.tsx), the right
// panel and the prompt dock. Opening Studio reads what is saved and nothing else (again each
// time it is shown, and when Settings closes over it): lists are fetched only on *Get the
// list* or *Update model lists*. The right panel holds the model and its settings (B5), and the
// prompt dock the added pictures, the prompt, the changed settings and Create (B7,
// PromptDock.tsx). The card stream holds the tab's runs as RunCards (B8), like chat messages:
// oldest at the top and newest at the bottom, just above the prompt dock (design 1, Wireframe
// 1): the gallery's runs read so far (the tab's own gallery, read again when the tab changes)
// with each run's newest state, and the runs started or heard of since. *Load more* at the top
// reads the gallery's next page, older runs, without moving what is shown; under the newest
// card, the queue strip counts this session's runs of the tab by state. The stream opens at its
// newest card, and a press accepted from Studio (Create, Again, Run again) brings its new card
// into view. With Grid on, the gallery's grid takes the
// stream's place (B9), and a result opened from a card, a tile or the chat's notice shows in the
// viewer over the whole view. While `localView` is on, Get local models (B13, LocalModels.tsx)
// takes the middle column's place, notices and empty states included, and the top bar hides
// its tabs and the gallery's controls; Back brings them all back.

/** The card stream of `tab`: the gallery's runs with each run's newest state, and the runs
 *  reported since (`live`) that it has not read, oldest first (the newest just above the dock). */
function streamOf(runs: Run[], live: Record<string, Run>, tab: StudioTab): Run[] {
  const byId = new Map<string, Run>();
  for (const run of [...Object.values(live), ...runs]) if (run.kind === tab && !byId.has(run.id)) byId.set(run.id, run);
  return [...byId.values()].sort((a, b) => a.createdAt - b.createdAt);
}

/** The card of run `id` brought into view, when it is shown. */
function showCard(id: string) {
  const card = document.querySelector(`[data-run-card="${CSS.escape(id)}"]`);
  card?.scrollIntoView?.({ block: "nearest" });
}

export function StudioView() {
  const tab = useStudio((s) => s.tab);
  const setTab = useStudio((s) => s.setTab);
  const loaded = useStudio((s) => s.loaded);
  const shown = useStudio((s) => s.view === "studio");
  const runs = useStudio((s) => s.runs);
  const live = useStudio((s) => s.live);
  const grid = useStudio((s) => s.grid);
  const localView = useStudio((s) => s.localView);
  const viewer = useStudio((s) => s.viewer);
  const openViewer = useStudio((s) => s.openViewer);
  const more = useStudio((s) => s.next !== null);
  const startedRun = useStudio((s) => s.startedRun);
  const stream = useMemo(() => streamOf(runs, live, tab), [runs, live, tab]);
  const scroller = useRef<HTMLElement>(null);
  const queue = useMemo(() => queueSummary(Object.values(live).filter((r) => r.kind === tab)), [live, tab]);
  // The banner's error, and whether a read of what is saved gave it: a read that works puts
  // that one away, but not an action's (a failed *Get the list* stays until dismissed).
  const [error, setError] = useState<{ info: ErrorInfo; read: boolean } | null>(null);
  const fail = (e: unknown) => setError({ info: toErrorInfo(e), read: false });
  // A picture dropped from the file manager on the tray or a frame zone, through Tauri's native
  // drop (the webview's own carries no files on Windows), while Studio is shown.
  useNativePictureDrops(shown);

  // What is saved, each time Studio is shown (App keeps it mounted while the chat is shown)
  // and when Settings closes over it: an endpoint or a preference may have changed there. The
  // first read takes everything, and is done again until it works; later ones take the lists,
  // local models, preferences and spending. All are reads of what is saved, never a fetch.
  // Outside the app there is nothing to read.
  useEffect(() => {
    if (!ipc.isTauri() || !shown) return;
    const read = () => {
      const s = useStudio.getState();
      (s.loaded ? s.reread() : s.load()).then(
        () => setError((now) => (now?.read ? null : now)),
        (e: unknown) => setError({ info: toErrorInfo(e), read: true }),
      );
    };
    read();
    window.addEventListener("app:settings-closed", read);
    return () => window.removeEventListener("app:settings-closed", read);
  }, [shown]);

  // The cards read the tab's own gallery: a tab changed (by its tab, Reuse, Animate, a viewer's
  // Again) reads its first page. The grid has its own filters.
  useEffect(() => {
    if (!loaded || grid) return;
    const s = useStudio.getState();
    if (s.filter.kind !== tab) s.setFilter({ kind: tab }).catch(fail);
  }, [loaded, grid, tab]);

  const cards = !localView && !grid;
  // The stream opens at its newest card, just above the dock, as a chat does: when Studio has
  // read what is saved, on another tab, and back from the grid or Get local models.
  useLayoutEffect(() => {
    const el = scroller.current;
    if (el && cards && loaded) el.scrollTop = el.scrollHeight;
  }, [cards, loaded, tab, shown]);

  // A press accepted from Studio brings its new card into view (once each).
  const scrolledTo = useRef<string | null>(null);
  useEffect(() => {
    if (!cards || startedRun === null || scrolledTo.current === startedRun) return;
    if (!stream.some((r) => r.id === startedRun)) return;
    scrolledTo.current = startedRun;
    showCard(startedRun);
  }, [cards, startedRun, stream]);

  // *Load more* puts older cards above what is shown, which stays where it was: its distance
  // from the bottom is kept until the older page is in (the oldest card shown changes).
  const keep = useRef<{ fromBottom: number; oldest: string | undefined } | null>(null);
  useLayoutEffect(() => {
    const el = scroller.current;
    const kept = keep.current;
    if (!el || !kept || stream[0]?.id === kept.oldest) return;
    keep.current = null;
    el.scrollTop = el.scrollHeight - kept.fromBottom;
  }, [stream]);
  const loadMore = () => {
    const el = scroller.current;
    if (el) keep.current = { fromBottom: el.scrollHeight - el.scrollTop, oldest: stream[0]?.id };
    useStudio
      .getState()
      .loadMore()
      .catch((e: unknown) => {
        keep.current = null;
        fail(e);
      });
  };

  return (
    <Tabs.Root
      value={tab}
      onValueChange={(value) => setTab(value as StudioTab)}
      className="flex min-w-0 flex-1 flex-col"
      data-studio-view
    >
      <TopBar onError={fail} />
      <Tabs.Content value={tab} className="flex min-h-0 flex-1 flex-col outline-none">
        <div className="flex min-h-0 flex-1">
          <main ref={scroller} className="min-w-0 flex-1 overflow-y-auto">
            <div className="mx-auto flex min-h-full max-w-3xl flex-col gap-3 px-4 py-4">
              {error && <ErrorBanner error={error.info} onDismiss={() => setError(null)} />}
              {!localView && loaded && <Notices tab={tab} onError={fail} />}
              {localView ? (
                <LocalModels />
              ) : grid ? (
                <Grid onOpen={openViewer} />
              ) : (
                // The cards sit just above the dock, however few there are.
                <div data-card-stream className="mt-auto flex flex-col gap-3">
                  {more && (
                    <button type="button" className={`${CARD_BUTTON} self-center`} onClick={loadMore}>
                      {STUDIO_GRID.loadMore}
                    </button>
                  )}
                  {stream.map((run) => (
                    <RunCard key={run.id} run={run} />
                  ))}
                  {queue && (
                    <p data-queue-strip className="text-xs text-fg-muted" aria-live="polite">
                      {queue}
                    </p>
                  )}
                </div>
              )}
            </div>
          </main>
          {/* The right panel: the model and its settings. */}
          <aside data-studio-panel className="w-72 shrink-0 overflow-y-auto border-l border-border">
            <SettingsPanel tab={tab} />
          </aside>
        </div>
        <PromptDock tab={tab} />
      </Tabs.Content>
      {viewer !== null && <Viewer outputId={viewer} onClose={() => openViewer(null)} />}
    </Tabs.Root>
  );
}
