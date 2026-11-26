import { useEffect, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore, type KeyboardEvent } from "react";
import { Tabs } from "radix-ui";
import { X } from "lucide-react";
import * as ipc from "../../lib/ipc";
import type { ErrorInfo } from "../../lib/types";
import { STUDIO_GRID, STUDIO_PANEL, UI } from "../../lib/words";
import { toErrorInfo } from "../../store/app";
import { DOCK_MIN_WIDTH, followStudioDownloads, useStudio, type StudioTab } from "../../store/studio";
import { ErrorBanner } from "../ErrorBanner";
import { SlideOver } from "../ui/SlideOver";
import { FOCUS_RING } from "../ui/styles";
import { Composer } from "./Composer";
import { Notices } from "./EmptyStates";
import { GetModelsView } from "./GetModels";
import { Grid } from "./Grid";
import { ImportDialog } from "./ImportDialog";
import { useNativePictureDrops } from "./RefTray";
import { queueSummary, RunCard } from "./RunCard";
import { SettingsPanel } from "./SettingsPanel";
import { CARD_BUTTON } from "./styles";
import { TopBar } from "./TopBar";
import type { Run } from "./types";
import { Viewer } from "./Viewer";

// Studio's view (0.1.0, design Wireframe 1; design 10.1 since 0.2.0 Task U3): the top bar
// (TopBar.tsx), then the results, which take the page: the card stream with the notices and
// empty states above it, local first (EmptyStates.tsx), and at its foot the composer
// (Composer.tsx), which holds the pictures, the prompt, the model chip, Shape, How many,
// Settings and Create. The side panel (SettingsPanel.tsx) stays closed until Settings opens it;
// on a window at least 1400 px wide it is docked beside the results and stays open while the
// person leaves it open (saved in the preferences), and on a narrower one it slides over them.
// Opening Studio reads what is saved and nothing else (again each time it is shown, and when
// Settings closes over it): lists are fetched only on *Get the list* or *Update model lists*.
// The card stream holds the tab's runs as RunCards (B8), like chat messages: oldest at the top
// and newest at the bottom, just above the composer: the gallery's runs read so far (the tab's
// own gallery, read again when the tab changes) with each run's newest state, and the runs
// started or heard of since. *Load more* at the top reads the gallery's next page, older runs,
// without moving what is shown; under the newest card, the queue strip counts this session's
// runs of the tab by state. The stream opens at its newest card, and a press accepted from
// Studio (Create, Again, Run again) brings its new card into view. With Grid on, the gallery's
// grid takes the stream's place (B9), and a result opened from a card, a tile or the chat's
// notice shows in the viewer over the whole view. While `localView` is on, Get models (Task U5,
// GetModels.tsx) is a full view: it takes the whole page beside the top bar, without the
// composer or the side panel, and opens at its top (critique §2 and §6: it kept the stream's
// scroll and opened at its bottom); the top bar hides its tabs and the gallery's controls; Back
// brings them all back, the stream at its newest card. *Add a model file* (ImportDialog.tsx,
// Task U4) opens over the view from the model sheet or Get models. From the moment Studio first
// opens it follows the app's downloads (Task U7), so a model done while Get models is closed, or
// while the chat is shown, is listed at once, and the top bar's indicator offers *Use it*.

/** Whether the window is at least `DOCK_MIN_WIDTH` wide, followed as it is resized. */
function useWideWindow(): boolean {
  return useSyncExternalStore(
    (changed) => {
      window.addEventListener("resize", changed);
      return () => window.removeEventListener("resize", changed);
    },
    () => window.innerWidth >= DOCK_MIN_WIDTH,
  );
}

/** The card stream of `tab`: the gallery's runs with each run's newest state, and the runs
 *  reported since (`live`) that it has not read, oldest first (the newest just above the composer). */
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
  // A picture dropped from the file manager on the composer or an input zone, through Tauri's
  // native drop (the webview's own carries no files on Windows), while Studio is shown.
  useNativePictureDrops(shown);
  const wide = useWideWindow();
  const settingsOpen = useStudio((s) => s.settingsOpen);
  const setSettingsOpen = useStudio((s) => s.setSettingsOpen);
  const docked = wide && settingsOpen;
  // Whether the docked panel was opened here (focus goes into it), not left open last time and
  // shown as Studio read what is saved (focus stays where it is).
  // Back from Get models, a panel left open is shown again where it was, without taking focus.
  const [seenPanel, setSeenPanel] = useState({ open: settingsOpen, loaded, localView });
  const [panelOpenedHere, setPanelOpenedHere] = useState(false);
  if (seenPanel.open !== settingsOpen || seenPanel.loaded !== loaded || seenPanel.localView !== localView) {
    setPanelOpenedHere(settingsOpen && !seenPanel.open && seenPanel.loaded);
    setSeenPanel({ open: settingsOpen, loaded, localView });
  }

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

  // Studio's downloads, followed from the moment Studio first opens for as long as the app runs
  // (App keeps this view mounted), whatever it shows: a model or LoRA that is done is read again,
  // and the cards, the detail panel and the top bar's indicator show how far each got (Task U7).
  useEffect(() => followStudioDownloads(), []);

  // The cards read the tab's own gallery: a tab changed (by its tab, Reuse, Animate, a viewer's
  // Again) reads its first page. The grid has its own filters.
  useEffect(() => {
    if (!loaded || grid) return;
    const s = useStudio.getState();
    if (s.filter.kind !== tab) s.setFilter({ kind: tab }).catch(fail);
  }, [loaded, grid, tab]);

  const cards = !localView && !grid;
  // The stream opens at its newest card, just above the composer, as a chat does: when Studio has
  // read what is saved, on another tab, and back from the grid or Get models.
  useLayoutEffect(() => {
    const el = scroller.current;
    if (el && cards && loaded) el.scrollTop = el.scrollHeight;
  }, [cards, loaded, tab, shown]);
  // Get models opens at its top, whatever the stream's scroll was.
  useLayoutEffect(() => {
    const el = scroller.current;
    if (el && localView) el.scrollTop = 0;
  }, [localView]);

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
      className="studio-page flex min-w-0 flex-1 flex-col"
      data-studio-view
    >
      <TopBar onError={fail} />
      <Tabs.Content value={tab} className="flex min-h-0 flex-1 outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-accent/40">
        <div data-results className="flex min-w-0 flex-1 flex-col">
          <main ref={scroller} className="min-h-0 flex-1 overflow-y-auto">
            {localView ? (
              <>
                {error && (
                  <div className="mx-auto max-w-5xl px-6 pt-4 max-sm:px-4">
                    <ErrorBanner error={error.info} onDismiss={() => setError(null)} />
                  </div>
                )}
                <GetModelsView />
              </>
            ) : (
              <div className="mx-auto flex min-h-full max-w-4xl flex-col gap-3 px-4 pt-4 pb-3">
                {error && <ErrorBanner error={error.info} onDismiss={() => setError(null)} />}
                {loaded && <Notices tab={tab} onError={fail} />}
                {grid ? (
                  <Grid onOpen={openViewer} />
                ) : (
                  // The cards sit just above the composer, however few there are.
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
            )}
          </main>
          {/* Get models is a full view: no composer, and no side panel below. */}
          {!localView && (
            <div className="shrink-0 px-4 pt-1 pb-3">
              <Composer tab={tab} />
            </div>
          )}
        </div>
        {docked && !localView && <DockedPanel tab={tab} focusIn={panelOpenedHere} onClose={() => setSettingsOpen(false)} />}
      </Tabs.Content>
      {!wide && !localView && (
        <SlideOver open={settingsOpen} onOpenChange={setSettingsOpen} title={STUDIO_PANEL.title} width="panel">
          <SettingsPanel tab={tab} />
        </SlideOver>
      )}
      {viewer !== null && <Viewer outputId={viewer} onClose={() => openViewer(null)} />}
      <ImportDialog />
    </Tabs.Root>
  );
}

/**
 * The side panel docked beside the results on a wide window: Settings with its Close button.
 * Opened here (`focusIn`), focus moves into it, and goes back to what had it as it closes (else
 * to the composer's Settings); Esc inside it closes it. It does not hold focus in: the page
 * beside it stays usable.
 */
function DockedPanel(props: { tab: StudioTab; focusIn: boolean; onClose(): void }) {
  const close = useRef<HTMLButtonElement>(null);
  const focusIn = useRef(props.focusIn);
  useEffect(() => {
    const opener = focusIn.current ? document.activeElement : null;
    if (focusIn.current) close.current?.focus();
    return () => {
      // Back where focus was, unless it has gone somewhere else meanwhile.
      const now = document.activeElement;
      if (now !== null && now !== document.body) return;
      const back = opener instanceof HTMLElement && opener.isConnected ? opener : document.querySelector("[data-settings-button]");
      if (back instanceof HTMLElement) back.focus();
    };
  }, []);
  const onKeyDown = (e: KeyboardEvent<HTMLElement>) => {
    // Not an Esc meant for a menu or a popup opened from inside it (drawn elsewhere).
    if (e.key !== "Escape" || !(e.target instanceof Node) || !e.currentTarget.contains(e.target)) return;
    e.preventDefault();
    props.onClose();
  };
  return (
    <aside
      aria-label={STUDIO_PANEL.title}
      data-studio-panel
      onKeyDown={onKeyDown}
      className="flex w-[22rem] shrink-0 flex-col border-l border-border bg-surface"
    >
      <div className="flex items-center justify-between gap-3 border-b border-border px-4 py-3">
        <h2 className="text-title">{STUDIO_PANEL.title}</h2>
        <button
          ref={close}
          type="button"
          aria-label={UI.close}
          onClick={props.onClose}
          className={`rounded-lg p-1.5 text-fg-muted hover:bg-bg-muted hover:text-fg ${FOCUS_RING}`}
        >
          <X className="size-4" aria-hidden />
        </button>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto p-4">
        <SettingsPanel tab={props.tab} />
      </div>
    </aside>
  );
}
