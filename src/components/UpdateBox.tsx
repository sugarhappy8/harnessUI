import { useEffect, useId, useState } from "react";
import { Download } from "lucide-react";
import * as ipc from "../lib/ipc";
import { listenInEffect } from "../lib/listen";
import { APP_NAME } from "../lib/app";
import type { ErrorInfo, UpdateInfo } from "../lib/types";
import { toErrorInfo, turnChatId, useApp } from "../store/app";
import { ErrorBanner } from "./ErrorBanner";

/**
 * Where the box floats. It never covers the chat's message box, its send button or the
 * header's controls:
 * - "sidebar": over the bottom of the chat list, above the model chip at the foot of the
 *   sidebar. The box is narrower than the sidebar, so it stays out of the chat.
 * - "below-header": the sidebar is hidden, so the bottom left belongs to the message box.
 *   The box goes under the chat's header bar, on the left, over the top of the messages.
 * - "corner": the bottom left corner of the window (the first-run screen, which has no
 *   message box there).
 */
export type UpdatePlacement = "sidebar" | "below-header" | "corner";

/** "Later" lasts until the app starts again: kept for this run of the window only. */
const LATER_KEY = "updateLater";

function readLater(): string | null {
  try {
    return window.sessionStorage.getItem(LATER_KEY);
  } catch {
    return null;
  }
}

function writeLater(version: string) {
  try {
    window.sessionStorage.setItem(LATER_KEY, version);
  } catch {
    // Then it lasts while this screen is open.
  }
}

/** The bottom edge of the chat's header bar, followed while the header wraps onto more rows,
 *  the window changes size or a bar appears above it. `null` until it is known. */
function useHeaderBottom(active: boolean): number | null {
  const [bottom, setBottom] = useState<number | null>(null);
  useEffect(() => {
    if (!active) return;
    const header = document.querySelector("main > header");
    if (!header) return;
    const measure = () => setBottom(header.getBoundingClientRect().bottom);
    measure();
    // The header's own size changes when it wraps; the row it sits in shrinks when a bar
    // appears above it (the stop hotkey warning) and when the window changes size.
    const row = header.parentElement?.parentElement;
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(measure);
    observer?.observe(header);
    if (row) observer?.observe(row);
    window.addEventListener("resize", measure);
    return () => {
      observer?.disconnect();
      window.removeEventListener("resize", measure);
    };
  }, [active]);
  return bottom;
}

/**
 * The little box that says a new version is ready (0.0.8). The app checks github.com on its
 * own (unless that is turned off in Settings → General → Updates), then downloads a newer
 * version and checks its signature in the background. This box appears once that is done.
 * Nothing is installed until the user clicks "Restart and update". "Later" hides the box
 * until the app starts again; a newer version that is ready later shows it again.
 */
export function UpdateBox(props: { placement: UpdatePlacement }) {
  const [ready, setReady] = useState<UpdateInfo | null>(null);
  const [later, setLater] = useState<string | null>(readLater);

  useEffect(() => {
    if (!ipc.isTauri()) return;
    let alive = true;
    const stop = listenInEffect(() => ipc.onUpdateReady((u) => alive && setReady(u)));
    // Events are not replayed: the version may have been ready before this window listened.
    ipc
      .updateStatus()
      .then((s) => {
        const found = s.ready;
        if (alive && found) setReady((cur) => cur ?? found);
      })
      .catch(() => undefined);
    return () => {
      alive = false;
      stop();
    };
  }, []);

  if (ready === null || ready.version === later) return null;
  return (
    <ReadyBox
      // A newer version starts afresh: notes closed, no old error.
      key={ready.version}
      update={ready}
      placement={props.placement}
      onLater={() => {
        setLater(ready.version);
        writeLater(ready.version);
      }}
    />
  );
}

const PLACES: Record<UpdatePlacement, string> = {
  sidebar: "bottom-[4.75rem] left-2 w-60",
  "below-header": "left-3 w-72",
  corner: "bottom-4 left-4 w-72",
};

function ReadyBox(props: { update: UpdateInfo; placement: UpdatePlacement; onLater: () => void }) {
  const { update } = props;
  const [notesOpen, setNotesOpen] = useState(false);
  const [installing, setInstalling] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const replying = useApp((s) => turnChatId(s) !== null);
  const headerBottom = useHeaderBottom(props.placement === "below-header");
  const notesId = useId();
  const notes = update.notes?.trim() ? update.notes : null;

  async function install() {
    setInstalling(true);
    setError(null);
    try {
      // On success the app restarts and this never returns.
      await ipc.installUpdate();
    } catch (err) {
      setError(toErrorInfo(err));
    } finally {
      setInstalling(false);
    }
  }

  return (
    <section
      role="status"
      aria-label="Update ready"
      data-update-box
      className={`enter fixed z-20 max-h-[calc(100vh-2rem)] overflow-y-auto rounded-xl border border-border bg-surface p-3 text-sm shadow-float ${PLACES[props.placement]}`}
      style={props.placement === "below-header" ? { top: `${(headerBottom ?? 104) + 8}px` } : undefined}
    >
      <div className="flex items-start gap-2">
        <Download size={15} className="mt-0.5 shrink-0 text-accent" aria-hidden />
        <div className="min-w-0 flex-1">
          <h2 className="font-medium">Update ready</h2>
          <p className="text-fg-muted">
            {APP_NAME} {update.version} is ready to install.
          </p>
          {replying && <p className="mt-1 text-xs text-fg-muted">Updating stops the current reply.</p>}
        </div>
      </div>
      {notes && notesOpen && (
        <div
          id={notesId}
          role="region"
          aria-label="Release notes"
          tabIndex={0}
          className="mt-2 max-h-40 overflow-y-auto rounded-md border border-border bg-bg-subtle p-2 text-xs break-words whitespace-pre-wrap text-fg-muted"
        >
          {notes}
        </div>
      )}
      <div className="mt-2.5 flex flex-wrap items-center gap-2">
        <button
          type="button"
          disabled={installing}
          onClick={() => void install()}
          className="rounded-md bg-accent px-3 py-1 font-medium text-accent-fg hover:opacity-90 disabled:opacity-60"
        >
          {installing ? "Updating…" : "Restart and update"}
        </button>
        <button
          type="button"
          disabled={installing}
          onClick={props.onLater}
          className="rounded-md border border-border px-3 py-1 hover:bg-bg-muted disabled:opacity-40"
        >
          Later
        </button>
        {notes && (
          <button
            type="button"
            aria-expanded={notesOpen}
            aria-controls={notesOpen ? notesId : undefined}
            onClick={() => setNotesOpen((o) => !o)}
            className="text-xs text-fg-muted underline underline-offset-2 hover:text-fg"
          >
            What&apos;s new
          </button>
        )}
      </div>
      {error && (
        <div className="mt-2">
          <ErrorBanner error={error} onDismiss={() => setError(null)} />
        </div>
      )}
    </section>
  );
}
