import { useEffect, useState } from "react";
import { AlertTriangle } from "lucide-react";
import * as ipc from "../lib/ipc";
import type { HotkeyStatus } from "../lib/types";
import { UpdateBox, type UpdatePlacement } from "./UpdateBox";

/**
 * Notices for the whole main window: a bar at the top when the stop hotkey could not be
 * registered (ARCHITECTURE.md 4.2; stays until a later status says it works), and the small
 * floating box that says a new version is ready to install (`UpdateBox`; `updatePlacement`
 * says where it floats).
 */
export function AppNotices(props: { onOpenSettings: (tab: string) => void; updatePlacement: UpdatePlacement }) {
  const [hotkey, setHotkey] = useState<HotkeyStatus | null>(null);

  useEffect(() => {
    if (!ipc.isTauri()) return;
    let alive = true;
    const offs: (() => void)[] = [];
    const keep = (off: () => void) => {
      if (alive) offs.push(off);
      else off();
    };
    void ipc.onStopHotkeyStatus(setHotkey).then(keep);
    // Events are not replayed, so also read what happened before this window listened.
    // Before the hotkey is first registered the status has no accelerator; skip that one.
    ipc
      .stopHotkeyStatus()
      .then((s) => {
        if (alive && s.accelerator) setHotkey((cur) => cur ?? s);
      })
      .catch(() => undefined);
    return () => {
      alive = false;
      offs.forEach((off) => off());
    };
  }, []);

  const hotkeyFailed = hotkey !== null && hotkey.accelerator !== "" && !hotkey.registered;
  return (
    <>
      {hotkeyFailed && (
        <div className="shrink-0">
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
        </div>
      )}
      {/* Floats above the page (fixed), so it takes no room in the column. */}
      <UpdateBox placement={props.updatePlacement} />
    </>
  );
}
