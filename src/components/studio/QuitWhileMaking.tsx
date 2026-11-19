import { useEffect, useRef, useState } from "react";
import { AlertDialog } from "radix-ui";
import * as ipc from "../../lib/ipc";
import { listenInEffect } from "../../lib/listen";
import { STUDIO_QUIT } from "../../lib/words";
import { useStudio } from "../../store/studio";
import * as api from "./api";
import type { MediaKind, Run } from "./types";

// The question before quitting while a local job works (0.1.0 Task B8): the backend sends
// `studio-quit-requested`, instead of closing, when the main window is closed or the app quit
// while a picture or video is being made on this computer. *Keep working* closes the question
// (Esc too); *Quit and stop* stops the local runs and quits, sent once.

/** What the local job holding the computer makes: the one running, else one waiting; `null`
 *  when none is known (a window that reloaded heard nothing of it). */
function localKind(live: Record<string, Run>): MediaKind | null {
  const runs = Object.values(live).filter((r) => r.runsOn === "local");
  return (runs.find((r) => r.status === "running") ?? runs.find((r) => r.status === "queued"))?.kind ?? null;
}

/** Asks, on `studio-quit-requested`, whether to stop the local job and quit. */
export function QuitWhileMaking() {
  const [open, setOpen] = useState(false);
  const [quitting, setQuitting] = useState(false);
  // Sent once, however quickly it is pressed again.
  const sent = useRef(false);
  const kind = useStudio((s) => localKind(s.live));

  useEffect(() => {
    if (!ipc.isTauri()) return;
    return listenInEffect(() => api.onQuitRequested(() => setOpen(true)));
  }, []);

  const quit = () => {
    if (sent.current) return;
    sent.current = true;
    setQuitting(true);
    // The app quits once the local runs are stopped. Should that fail, it can be pressed again.
    api.studioQuitNow().catch((e: unknown) => {
      sent.current = false;
      setQuitting(false);
      console.warn("Studio: quitting failed", e);
    });
  };

  return (
    <AlertDialog.Root open={open} onOpenChange={setOpen}>
      <AlertDialog.Portal>
        <AlertDialog.Overlay className="dialog-overlay fixed inset-0" />
        <AlertDialog.Content className="fixed top-1/2 left-1/2 w-[26rem] max-w-[92vw] -translate-x-1/2 -translate-y-1/2 rounded-2xl border border-border bg-surface p-5 text-sm shadow-pop">
          <AlertDialog.Title className="text-base font-semibold">{STUDIO_QUIT.title}</AlertDialog.Title>
          <AlertDialog.Description className="mt-2 text-fg-muted">{STUDIO_QUIT.making(kind)}</AlertDialog.Description>
          <div className="mt-4 flex justify-end gap-2">
            <AlertDialog.Cancel className="btn-primary rounded-md px-3 py-1.5 font-medium">
              {STUDIO_QUIT.keepWorking}
            </AlertDialog.Cancel>
            {/* Not an AlertDialog.Action: the question stays until the app quits. */}
            <button
              type="button"
              disabled={quitting}
              onClick={quit}
              className="rounded-md border border-border px-3 py-1.5 hover:bg-bg-muted disabled:opacity-60"
            >
              {STUDIO_QUIT.quitAndStop}
            </button>
          </div>
        </AlertDialog.Content>
      </AlertDialog.Portal>
    </AlertDialog.Root>
  );
}
