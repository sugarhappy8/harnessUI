import { useEffect, useState } from "react";
import * as api from "./api";
import type { FrameInfo } from "./types";

/**
 * What the frame for one version loads, asked for when the frame is about to be created.
 *
 * A preview's address is good for one load (src-tauri/src/artifacts/protocol.rs,
 * `Tickets`), so every frame gets its own: when the version changes, and whenever `again`
 * does (the user reloads the preview or allows its scripts, or the sandbox check ends).
 * `null` while it is on its way, when there is nothing to frame, and when the app refused.
 */
export function useFrame(
  chatId: number,
  id: number | null,
  version: number | null,
  again: string | number,
): FrameInfo | null {
  const [got, setGot] = useState<{ key: string; info: FrameInfo } | null>(null);
  const key = id === null || version === null ? "" : `${chatId}:${id}:${version}:${again}`;
  useEffect(() => {
    if (id === null || version === null) return;
    let stale = false;
    api
      .artifactFrame(chatId, id, version)
      .then((info) => {
        if (!stale) setGot({ key, info });
      })
      .catch(() => {
        if (!stale) setGot(null);
      });
    return () => {
      stale = true;
    };
  }, [chatId, id, version, key]);
  return got && got.key === key ? got.info : null;
}

/** Whether anything runs in a frame served this way (and so whether it will post messages). */
export function runsScripts(frame: FrameInfo): boolean {
  return frame.running === "page" || frame.running === "appOnly";
}
