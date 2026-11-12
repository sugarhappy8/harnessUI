import { useEffect, useRef } from "react";
import { MAX_MESSAGES, addProblem, parseFrameMessage } from "./messages";
import type { ArtifactKind, Problem } from "./types";

/** After an HTML page has loaded, problems are collected this long before they are reported. */
const SETTLE_HTML_MS = 1500;
/** A picture or a diagram says itself when it is done; a moment for stragglers. */
const SETTLE_OTHER_MS = 300;
/** A page that never says it has loaded is reported with what there is by then. */
const GIVE_UP_MS = 3200;

/** How a preview went, once it has settled. */
export interface Settled {
  problems: Problem[];
  /** The page said it cannot draw this kind on this system. */
  unavailable: boolean;
}

/**
 * One version of an artifact in a sandboxed frame.
 *
 * The frame is the boundary between the app and a page the model wrote. Its address is of
 * a scheme of its own, served by Rust with a sandbox and rules that allow no request
 * (src-tauri/src/artifacts/protocol.rs); the element repeats the sandbox. The only thing
 * that comes out of it is `postMessage`. Messages are taken only from this frame's own
 * window, read by `parseFrameMessage`, and used as text.
 */
export function ArtifactFrame(props: {
  id: number;
  version: number;
  kind: ArtifactKind;
  url: string;
  title: string;
  /** Whether the page may run scripts (only after the sandbox has been checked). */
  scripts: boolean;
  /** Shown or kept out of sight (a preview rendered only to report on it). */
  hidden?: boolean;
  /** Changes when the user asks for the page to be loaded again. */
  reload?: number;
  onProblems: (problems: Problem[]) => void;
  onSettled: (outcome: Settled) => void;
}) {
  const { id, version, kind, url, scripts, reload } = props;
  const frame = useRef<HTMLIFrameElement>(null);
  // The newest handlers, without making the listener below start over when they change.
  const handlers = useRef({ onProblems: props.onProblems, onSettled: props.onSettled });
  useEffect(() => {
    handlers.current = { onProblems: props.onProblems, onSettled: props.onSettled };
  });

  useEffect(() => {
    if (!scripts) return;
    let problems: Problem[] = [];
    let seen = 0;
    let settled = false;
    let settleTimer: number | undefined;

    const settle = (unavailable: boolean) => {
      if (settled) return;
      settled = true;
      window.clearTimeout(settleTimer);
      window.clearTimeout(giveUp);
      handlers.current.onSettled({ problems, unavailable });
    };

    const onMessage = (event: MessageEvent) => {
      // Only this frame's own window. Another frame, or this window, is not a preview.
      const own = frame.current?.contentWindow;
      if (!own || event.source !== own) return;
      seen += 1;
      if (seen > MAX_MESSAGES) return;
      const message = parseFrameMessage(event.data, id, version);
      if (!message) return;
      if (message.kind === "problem") {
        const next = addProblem(problems, message.problem);
        if (next !== problems) {
          problems = next;
          handlers.current.onProblems(problems);
        }
      } else if (message.kind === "unavailable") {
        settle(true);
      } else if (settleTimer === undefined && !settled) {
        settleTimer = window.setTimeout(() => settle(false), kind === "html" ? SETTLE_HTML_MS : SETTLE_OTHER_MS);
      }
    };

    const giveUp = window.setTimeout(() => settle(false), GIVE_UP_MS);
    window.addEventListener("message", onMessage);
    return () => {
      window.removeEventListener("message", onMessage);
      window.clearTimeout(settleTimer);
      window.clearTimeout(giveUp);
    };
  }, [id, version, kind, url, scripts, reload]);

  return (
    <iframe
      // A new element for every page and every change of the sandbox: an element that
      // is kept keeps the document it loaded, with the rights it was loaded with.
      key={`${url}|${scripts ? "scripts" : "static"}|${reload ?? 0}`}
      ref={frame}
      src={url}
      title={props.title}
      sandbox={scripts ? "allow-scripts" : ""}
      referrerPolicy="no-referrer"
      data-artifact={`${id}:${version}`}
      aria-hidden={props.hidden ? true : undefined}
      tabIndex={props.hidden ? -1 : undefined}
      className={props.hidden ? "hidden" : "h-full w-full border-0 bg-white"}
    />
  );
}
