import { useApp } from "../../store/app";
import * as api from "./api";
import { useArtifacts } from "./state";
import type { ArtifactChanged, Isolation } from "./types";

// What runs once when the app's window starts: listening for artifacts the model makes,
// and checking the preview sandbox on this system.

/** True inside the app's window; false in a plain browser and in tests. */
export function inApp(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/** How long the check's page has to report. */
const CHECK_TIMEOUT_MS = 6000;
/** After its report: time for anything the page sent to the app to arrive there. */
const CHECK_GRACE_MS = 700;

const UNFINISHED: Isolation = { state: "broken", reason: "The check of the preview did not finish." };

/** A tool made or changed an artifact: show it when its chat is the one on screen. */
export function onArtifactChanged(c: ArtifactChanged): void {
  const artifacts = useArtifacts.getState();
  artifacts.noteChange(c);
  const app = useApp.getState();
  if (app.activeChatId !== c.chatId) return;
  artifacts.requestFocus(c.chatId, c.id, c.version);
  app.openWorkbench("artifacts");
}

/** Only these four fields of what the check's page posts go to Rust, as short strings. */
function reportFields(value: unknown): Record<string, string> | null {
  if (typeof value !== "object" || value === null) return null;
  const v = value as Record<string, unknown>;
  const text = (x: unknown) => (typeof x === "string" ? x.slice(0, 100) : "");
  return { origin: text(v.origin), storage: text(v.storage), parent: text(v.parent), cookie: text(v.cookie) };
}

let running: Promise<Isolation> | null = null;

/**
 * Checks the preview sandbox: loads a page of the app's own into a hidden frame from the
 * address previews use, and lets it try to reach the app and the network. Rust decides
 * from what arrived there and from what the page reports (isolation.rs). Previews run
 * scripts only after this has passed.
 */
export function runIsolationCheck(): Promise<Isolation> {
  running ??= check().finally(() => {
    running = null;
  });
  return running;
}

async function check(): Promise<Isolation> {
  const { setIsolation } = useArtifacts.getState();
  setIsolation({ state: "checking" });
  try {
    const { nonce, url } = await api.beginArtifactCheck();
    const frame = document.createElement("iframe");
    frame.setAttribute("sandbox", "allow-scripts");
    frame.setAttribute("referrerpolicy", "no-referrer");
    frame.setAttribute("aria-hidden", "true");
    frame.setAttribute("title", "Preview check");
    frame.tabIndex = -1;
    frame.style.display = "none";
    const report = await new Promise<unknown>((resolve) => {
      const finish = (value: unknown) => {
        window.clearTimeout(timer);
        window.removeEventListener("message", onMessage);
        resolve(value);
      };
      const onMessage = (event: MessageEvent) => {
        // Only the check's own frame, and only its message for this check.
        if (event.source !== frame.contentWindow) return;
        const data: unknown = event.data;
        if (typeof data !== "object" || data === null) return;
        const d = data as Record<string, unknown>;
        if (d.source !== "artifact-probe" || d.nonce !== nonce) return;
        finish(d.report ?? null);
      };
      const timer = window.setTimeout(() => finish(null), CHECK_TIMEOUT_MS);
      window.addEventListener("message", onMessage);
      frame.src = url;
      document.body.appendChild(frame);
    });
    await new Promise((resolve) => window.setTimeout(resolve, CHECK_GRACE_MS));
    frame.remove();
    const verdict = await api.finishArtifactCheck(nonce, reportFields(report));
    setIsolation(verdict);
    return verdict;
  } catch {
    setIsolation(UNFINISHED);
    return UNFINISHED;
  }
}

let started = false;

/** Starts listening and checking. Runs once, when the app's window loads. */
export function startArtifacts(): void {
  if (started || !inApp()) return;
  started = true;
  void api.onArtifactChanged(onArtifactChanged);
  void api.onArtifactIsolation((i) => useArtifacts.getState().setIsolation(i));
  void api
    .artifactIsolation()
    .then((known) => {
      // After a reload of the window the app still knows the result of its check.
      if (known.state === "proven") useArtifacts.getState().setIsolation(known);
      else void runIsolationCheck();
    })
    .catch(() => useArtifacts.getState().setIsolation(UNFINISHED));
}
