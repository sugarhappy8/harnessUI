import { useEffect, useMemo, useState } from "react";
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
// PromptDock.tsx). The card stream holds the tab's runs, newest first, as RunCards (B8): the
// gallery's runs read so far with each run's newest state, and the runs started or heard of
// since; above them, the queue strip counts this session's runs of the tab by state, and under
// them *Load more* reads the gallery's next page. With Grid on, the gallery's grid takes the
// stream's place (B9), and a result opened from a card, a tile or the chat's notice shows in the
// viewer over the whole view. While `localView` is on, Get local models (B13, LocalModels.tsx)
// takes the middle column's place, notices and empty states included, and the top bar hides
// its tabs and the gallery's controls; Back brings them all back.

/** The card stream of `tab`: the gallery's runs with each run's newest state, and the runs
 *  reported since (`live`) that it has not read, newest first. */
function streamOf(runs: Run[], live: Record<string, Run>, tab: StudioTab): Run[] {
  const byId = new Map<string, Run>();
  for (const run of [...Object.values(live), ...runs]) if (run.kind === tab && !byId.has(run.id)) byId.set(run.id, run);
  return [...byId.values()].sort((a, b) => b.createdAt - a.createdAt);
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
  const stream = useMemo(() => streamOf(runs, live, tab), [runs, live, tab]);
  const queue = useMemo(() => queueSummary(Object.values(live).filter((r) => r.kind === tab)), [live, tab]);
  // The banner's error, and whether a read of what is saved gave it: a read that works puts
  // that one away, but not an action's (a failed *Get the list* stays until dismissed).
  const [error, setError] = useState<{ info: ErrorInfo; read: boolean } | null>(null);
  const fail = (e: unknown) => setError({ info: toErrorInfo(e), read: false });

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
          <main className="min-w-0 flex-1 overflow-y-auto">
            <div className="mx-auto flex max-w-3xl flex-col gap-3 px-4 py-4">
              {error && <ErrorBanner error={error.info} onDismiss={() => setError(null)} />}
              {!localView && loaded && <Notices tab={tab} onError={fail} />}
              {localView ? (
                <LocalModels />
              ) : grid ? (
                <Grid onOpen={openViewer} />
              ) : (
                <div data-card-stream className="flex flex-col gap-3">
                  {queue && (
                    <p data-queue-strip className="text-xs text-fg-muted" aria-live="polite">
                      {queue}
                    </p>
                  )}
                  {stream.map((run) => (
                    <RunCard key={run.id} run={run} />
                  ))}
                  {more && (
                    <button
                      type="button"
                      className={`${CARD_BUTTON} self-center`}
                      onClick={() => void useStudio.getState().loadMore().catch(fail)}
                    >
                      {STUDIO_GRID.loadMore}
                    </button>
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
