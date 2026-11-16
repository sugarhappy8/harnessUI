import { isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { listenInEffect } from "./listen";

/**
 * [0.0.7] Sent by the app each time it brought the main window back on screen after the
 * window stepped aside for computer use: for an approval card, at the end of the turn, or
 * on Stop (`src-tauri/src/computer_cmd/aside.rs`).
 */
export const MAIN_WINDOW_SHOWN = "main-window-shown";

/**
 * Calls `handler` each time the app brought the main window back; returns what stops it,
 * for an effect's cleanup. The handler can still be called once after the cleanup (see
 * `listenInEffect`). Outside the app it does nothing.
 */
export function onMainWindowShown(handler: () => void): () => void {
  if (!isTauri()) return () => undefined;
  return listenInEffect(() => listen(MAIN_WINDOW_SHOWN, () => handler()));
}
