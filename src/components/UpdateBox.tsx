import { useEffect, useId, useRef, useState, type RefObject } from "react";
import { Download } from "lucide-react";
import * as ipc from "../lib/ipc";
import { listenInEffect } from "../lib/listen";
import { APP_NAME } from "../lib/app";
import type { ErrorInfo, UpdateInfo } from "../lib/types";
import { toErrorInfo, turnChatId, useApp } from "../store/app";
import { ErrorBanner } from "./ErrorBanner";

/**
 * Where the box floats. It never covers the chat's message box, its send button or the
 * header's controls, and its height is limited to the room it has there (`useRoom`):
 * - "sidebar": over the bottom of the chat list, above the model chip at the foot of the
 *   sidebar. The box is narrower than the sidebar, so it stays out of the chat. It grows up
 *   no further than the top of the chat list, clear of New chat and Search, and the list
 *   leaves room at its end so its last chats can be scrolled above the box.
 * - "below-header": the sidebar is hidden, so the bottom left belongs to the message box.
 *   The box goes under the chat's header bar, on the left, over the top of the messages,
 *   and ends above the message box.
 * - "corner": the bottom left corner of the window (the first-run screen, which has no
 *   message box there). The first-run screen leaves room at its end, so its buttons can be
 *   scrolled above the box.
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

/** The space the box keeps from the controls around it, in pixels. */
const GAP = 8;

/** The header's bottom edge until it is measured, in pixels. */
const HEADER_FALLBACK = 104;

/** The least height the box is given, in pixels, however little room the window has. */
const MIN_HEIGHT = 80;

/**
 * A CSS variable on the page root: how much of the bottom of what the box floats over it
 * covers. The chat list (Sidebar.tsx) and the first-run screen (FirstRun.tsx) add it to their
 * bottom padding, so their last items can be scrolled clear of the box. Unset (0) without the
 * box.
 */
export const UPDATE_BOX_SPACE = "--update-box-space";

type Room = { top: number | null; maxHeight: number | null };

/**
 * Where the box may be, measured on the page: its top edge under the chat's header
 * ("below-header"), and the most height it may take without covering the controls around it
 * (`null`: the window's height less 1rem above and below). Followed while the header wraps,
 * the message box grows, a bar appears above, the box itself grows (release notes, an error)
 * or the window changes size. Also publishes [`UPDATE_BOX_SPACE`].
 */
function useRoom(placement: UpdatePlacement, box: RefObject<HTMLElement | null>): Room {
  const [room, setRoom] = useState<Room>({ top: null, maxHeight: null });
  useEffect(() => {
    const el = box.current;
    if (!el) return;
    const root = document.documentElement;
    const header = placement === "below-header" ? document.querySelector("main > header") : null;
    const composer = placement === "below-header" ? document.querySelector("[data-composer]") : null;
    const list = placement === "sidebar" ? document.querySelector('nav[aria-label="Chat history"]') : null;
    const measure = () => {
      const windowHeight = window.innerHeight;
      let next: Room = { top: null, maxHeight: null };
      if (placement === "below-header") {
        const top = (header ? header.getBoundingClientRect().bottom : HEADER_FALLBACK) + GAP;
        // Down to the message box, clear of it and its send button.
        const bottom = composer ? composer.getBoundingClientRect().top : windowHeight;
        next = { top, maxHeight: Math.max(MIN_HEIGHT, bottom - GAP - top) };
      } else if (placement === "sidebar" && list) {
        // The box sits at the foot of the sidebar and grows up: no higher than the top of the
        // chat list, so New chat and Search stay clear.
        const bottom = el.getBoundingClientRect().bottom;
        next = { top: null, maxHeight: Math.max(MIN_HEIGHT, bottom - GAP - list.getBoundingClientRect().top) };
      }
      setRoom((cur) => (cur.top === next.top && cur.maxHeight === next.maxHeight ? cur : next));
      // The part of the chat list, or of the first-run screen (which ends at the window's
      // bottom), the box covers.
      const under = placement === "sidebar" ? (list?.getBoundingClientRect().bottom ?? null) : placement === "corner" ? windowHeight : null;
      const space = under === null ? 0 : Math.max(0, Math.ceil(under - el.getBoundingClientRect().top + GAP));
      root.style.setProperty(UPDATE_BOX_SPACE, `${space}px`);
    };
    measure();
    // The header's own size changes when it wraps; the row it sits in shrinks when a bar
    // appears above it (the stop hotkey warning) and when the window changes size.
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(measure);
    for (const target of [el, header, header?.parentElement?.parentElement, composer, list]) {
      if (target) observer?.observe(target);
    }
    window.addEventListener("resize", measure);
    // The box rises into place as it appears, and a moving box measures where it is just
    // then: measure again once it has arrived.
    el.addEventListener("animationend", measure);
    return () => {
      observer?.disconnect();
      window.removeEventListener("resize", measure);
      el.removeEventListener("animationend", measure);
      root.style.removeProperty(UPDATE_BOX_SPACE);
    };
  }, [placement, box]);
  return room;
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

  const shown = ready !== null && ready.version !== later ? ready : null;
  return (
    <>
      {/* Says it to a screen reader once, when a version becomes ready. The box itself is not
          a live region, so what changes inside it later is not read out again. */}
      <div role="status" className="sr-only">
        {shown ? `Update ready. ${APP_NAME} ${shown.version} is ready to install.` : ""}
      </div>
      {shown && (
        <ReadyBox
          // A newer version starts afresh: notes closed, no old error.
          key={shown.version}
          update={shown}
          placement={props.placement}
          onLater={() => {
            setLater(shown.version);
            writeLater(shown.version);
            // The focused button goes away with the box: put the caret back in the message box.
            window.dispatchEvent(new Event("app:focus-input"));
          }}
        />
      )}
    </>
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
  const boxRef = useRef<HTMLElement>(null);
  const room = useRoom(props.placement, boxRef);
  const notesId = useId();
  const headingId = useId();
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
      ref={boxRef}
      role="region"
      aria-labelledby={headingId}
      data-update-box
      // A column, so the release notes shrink first when room is short; past that the box
      // scrolls.
      className={`enter fixed z-20 flex max-h-[calc(100vh-2rem)] flex-col overflow-y-auto rounded-xl border border-border bg-surface p-3 text-sm shadow-float ${PLACES[props.placement]}`}
      style={{
        top: props.placement === "below-header" ? `${room.top ?? HEADER_FALLBACK + GAP}px` : undefined,
        maxHeight: room.maxHeight === null ? undefined : `${room.maxHeight}px`,
      }}
    >
      <div className="flex shrink-0 items-start gap-2">
        <Download size={15} className="mt-0.5 shrink-0 text-accent" aria-hidden />
        <div className="min-w-0 flex-1">
          <h2 id={headingId} className="font-medium">
            Update ready
          </h2>
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
          className="mt-2 max-h-40 min-h-12 overflow-y-auto rounded-md border border-border bg-bg-subtle p-2 text-xs break-words whitespace-pre-wrap text-fg-muted"
        >
          {notes}
        </div>
      )}
      <div className="mt-2.5 flex shrink-0 flex-wrap items-center gap-2">
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
        <div className="mt-2 shrink-0">
          <ErrorBanner error={error} onDismiss={() => setError(null)} />
        </div>
      )}
    </section>
  );
}
