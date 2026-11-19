import { useEffect, useState } from "react";
import { Tabs } from "radix-ui";
import * as ipc from "../../lib/ipc";
import type { ErrorInfo } from "../../lib/types";
import { toErrorInfo } from "../../store/app";
import { useStudio, type StudioTab } from "../../store/studio";
import { ErrorBanner } from "../ErrorBanner";
import { Notices } from "./EmptyStates";
import { PromptDock } from "./PromptDock";
import { SettingsPanel } from "./SettingsPanel";
import { TopBar } from "./TopBar";

// Studio's view (0.1.0, design Wireframe 1): the top bar (TopBar.tsx), the card stream in the
// middle with the notices and empty states above it, local first (EmptyStates.tsx), the right
// panel and the prompt dock. Opening Studio reads what is saved and nothing else (again each
// time it is shown, and when Settings closes over it): lists are fetched only on *Get the
// list* or *Update model lists*. The right panel holds the model and its settings (B5), and the
// prompt dock the added pictures, the prompt, the changed settings and Create (B7,
// PromptDock.tsx). Later tasks fill the other slots: the run cards (B8) and Get local models in
// place of the stream while `localView` is on (B13).

export function StudioView() {
  const tab = useStudio((s) => s.tab);
  const setTab = useStudio((s) => s.setTab);
  const loaded = useStudio((s) => s.loaded);
  const shown = useStudio((s) => s.view === "studio");
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
              {loaded && <Notices tab={tab} onError={fail} />}
              {/* The run cards (Task B8), or the gallery's grid (Task B9). */}
              <div data-card-stream />
            </div>
          </main>
          {/* The right panel: the model and its settings. */}
          <aside data-studio-panel className="w-72 shrink-0 overflow-y-auto border-l border-border">
            <SettingsPanel tab={tab} />
          </aside>
        </div>
        <PromptDock tab={tab} />
      </Tabs.Content>
    </Tabs.Root>
  );
}
