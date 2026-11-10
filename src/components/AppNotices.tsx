import { useEffect, useState } from "react";
import { AlertTriangle, Download, X } from "lucide-react";
import * as ipc from "../lib/ipc";
import type { HotkeyStatus, UpdateInfo } from "../lib/types";

/**
 * Bars at the top of the main window: a stop hotkey that could not be registered
 * (ARCHITECTURE.md 4.2; stays until a later status says it works) and an update found
 * by the check at start (can be dismissed).
 */
export function AppNotices(props: { onOpenSettings: (tab: string) => void }) {
  const [hotkey, setHotkey] = useState<HotkeyStatus | null>(null);
  const [update, setUpdate] = useState<UpdateInfo | null>(null);
  const [dismissed, setDismissed] = useState<string | null>(null);

  useEffect(() => {
    if (!ipc.isTauri()) return;
    let alive = true;
    const offs: (() => void)[] = [];
    const keep = (off: () => void) => {
      if (alive) offs.push(off);
      else off();
    };
    void ipc.onStopHotkeyStatus(setHotkey).then(keep);
    void ipc.onUpdateAvailable(setUpdate).then(keep);
    // Events are not replayed, so also read what happened before this window listened.
    // Before the hotkey is first registered the status has no accelerator; skip that one.
    ipc
      .stopHotkeyStatus()
      .then((s) => {
        if (alive && s.accelerator) setHotkey((cur) => cur ?? s);
      })
      .catch(() => undefined);
    ipc
      .updateStatus()
      .then((s) => {
        const found = s.available;
        if (alive && found) setUpdate((cur) => cur ?? found);
      })
      .catch(() => undefined);
    return () => {
      alive = false;
      offs.forEach((off) => off());
    };
  }, []);

  const hotkeyFailed = hotkey !== null && hotkey.accelerator !== "" && !hotkey.registered;
  const showUpdate = update !== null && update.version !== dismissed;
  if (!hotkeyFailed && !showUpdate) return null;
  return (
    <div className="shrink-0">
      {hotkeyFailed && (
        <div
          role="alert"
          className="flex items-center gap-2 border-b border-warning/60 bg-warning/10 px-4 py-1.5 text-sm"
        >
          <AlertTriangle size={14} className="shrink-0 text-warning" aria-hidden />
          <span className="min-w-0 flex-1">
            The stop hotkey could not be registered. Choose another one in Settings → Permissions.
          </span>
          <button type="button" className="text-xs underline" onClick={() => props.onOpenSettings("permissions")}>
            Open Permissions
          </button>
        </div>
      )}
      {showUpdate && update && (
        <div role="status" className="flex items-center gap-2 border-b border-border bg-bg-subtle px-4 py-1.5 text-sm">
          <Download size={14} className="shrink-0 text-fg-muted" aria-hidden />
          <span className="min-w-0 flex-1">Version {update.version} is available.</span>
          <button type="button" className="text-xs underline" onClick={() => props.onOpenSettings("general")}>
            Open Updates
          </button>
          <button
            type="button"
            aria-label="Dismiss update notice"
            onClick={() => setDismissed(update.version)}
            className="text-fg-muted hover:text-fg"
          >
            <X size={14} aria-hidden />
          </button>
        </div>
      )}
    </div>
  );
}
