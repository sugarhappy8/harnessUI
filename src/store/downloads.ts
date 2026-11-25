// The download groups as the app's download manager reports them (0.1.0 Task B14): what Studio
// reads to show a picture or video model's, or a LoRA's, download on its card, in the detail
// panel and in the top bar's indicator, with Pause and Cancel (0.2.0 Task U7; Models → Downloads
// keeps its own list). Followed while Studio is mounted (`followStudioDownloads`); reading them
// sends nothing anywhere.
import { create } from "zustand";
import * as ipc from "../lib/ipc";
import { listenInEffect } from "../lib/listen";
import type { DownloadGroup } from "../lib/types";

export interface DownloadsState {
  /** Every download group, newest first; `[]` until read. */
  groups: DownloadGroup[];
  /** Reads every group again. A group heard of while the list was on its way keeps what was
   *  heard (it is newer). Rejects when the list could not be read. */
  read(): Promise<void>;
  /** One group's new state, from its `download-progress` event. */
  put(group: DownloadGroup): void;
}

/** Counts the groups heard of, so a list asked for before an event never takes its place. */
let heard = 0;
const heardAt = new Map<string, number>();

export const useDownloads = create<DownloadsState>()((set) => ({
  groups: [],
  read: async () => {
    const asked = heard;
    const listed = await ipc.listDownloads();
    set((s) => {
      const newer = s.groups.filter((g) => (heardAt.get(g.id) ?? 0) > asked);
      const listedIds = new Set(listed.map((g) => g.id));
      return {
        groups: [
          ...newer.filter((g) => !listedIds.has(g.id)),
          ...listed.map((g) => newer.find((n) => n.id === g.id) ?? g),
        ],
      };
    });
  },
  put: (group) => {
    heardAt.set(group.id, ++heard);
    set((s) => {
      const i = s.groups.findIndex((g) => g.id === group.id);
      if (i < 0) return { groups: [group, ...s.groups] };
      const groups = s.groups.slice();
      groups[i] = group;
      return { groups };
    });
  },
}));

/** Follows the download groups: each `download-progress` event (`onChange` hears its group
 *  too) and, once listening, the list as it is now. Returns the function that stops following;
 *  outside the app it follows nothing. A list that cannot be read leaves the groups as they
 *  were, with a trace. */
export function followDownloads(onChange?: (group: DownloadGroup) => void): () => void {
  if (!ipc.isTauri()) return () => undefined;
  return listenInEffect(async () => {
    const unlisten = await ipc.onDownloadProgress((group) => {
      useDownloads.getState().put(group);
      onChange?.(group);
    });
    // Asked once listening has started, so nothing said in between is missed.
    useDownloads
      .getState()
      .read()
      .catch((e: unknown) => console.warn("The downloads could not be read", e));
    return unlisten;
  });
}
