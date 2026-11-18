// Studio's store (0.1.0): which view the main window shows, the chat or Studio (the chat
// stays mounted, and its reply keeps running, while Studio is shown), Studio's tab, and the
// runs as the backend reports them (`studio-progress`; `studio_live` once at start, as events
// are not replayed). A video that finishes while the chat is shown raises a notice there.
import { create, type StoreApi, type UseBoundStore } from "zustand";
import * as api from "../components/studio/api";
import type { MediaKind, Run, RunStatus } from "../components/studio/types";
import * as ipc from "../lib/ipc";
import { listenInEffect } from "../lib/listen";

/** Studio's tabs: Pictures and Video. */
export type StudioTab = MediaKind;

export interface StudioState {
  /** What the main window shows beside the chat list. */
  view: "chat" | "studio";
  tab: StudioTab;
  /** Each run the backend reported, by id, at its newest state. */
  live: Record<string, Run>;
  /** The id of a video that finished while the chat was shown. */
  readyNotice: string | null;
  /** Opens Studio, on `tab` when given (else on the tab shown last). */
  showStudio(tab?: StudioTab): void;
  showChat(): void;
  /** A run's new state, from `studio-progress` or `studio_live`. */
  applyProgress(run: Run): void;
}

export const useStudio: UseBoundStore<StoreApi<StudioState>> = create<StudioState>((set) => ({
  view: "chat",
  tab: "image",
  live: {},
  readyNotice: null,
  showStudio: (tab) => set((s) => ({ view: "studio", tab: tab ?? s.tab })),
  showChat: () => set({ view: "chat" }),
  applyProgress: (run) =>
    set((s) => {
      const finished = run.kind === "video" && run.status === "done" && s.live[run.id]?.status !== "done";
      return {
        live: { ...s.live, [run.id]: run },
        readyNotice: finished && s.view === "chat" ? run.id : s.readyNotice,
      };
    }),
}));

/** The states in which a run works whether or not Studio watches it. */
const WORKING: ReadonlySet<RunStatus> = new Set<RunStatus>(["queued", "sending", "running", "downloading"]);

/** The runs working now: queued, sending, running, downloading, and waiting while watched
 *  (a video no longer checked is not). */
export function runningCount(s: StudioState): number {
  return Object.values(s.live).filter((r) => WORKING.has(r.status) || (r.status === "waiting" && r.watching))
    .length;
}

/** Follows the runs: each `studio-progress` and, once listening has started, the runs still
 *  working now (a window that reloaded heard nothing of them). What an event says is newer
 *  than that answer. Returns the function that stops following; outside the app it follows
 *  nothing. */
export function followStudio(): () => void {
  if (!ipc.isTauri()) return () => undefined;
  const heard = new Set<string>();
  let stopped = false;
  const stopListening = listenInEffect(async () => {
    const unlisten = await api.onStudioProgress((run) => {
      if (stopped) return;
      heard.add(run.id);
      useStudio.getState().applyProgress(run);
    });
    if (!stopped) {
      Promise.resolve()
        .then(() => api.studioLive())
        .then((runs) => {
          if (stopped) return;
          for (const run of runs) if (!heard.has(run.id)) useStudio.getState().applyProgress(run);
        })
        .catch(() => undefined);
    }
    return unlisten;
  });
  return () => {
    stopped = true;
    stopListening();
  };
}
