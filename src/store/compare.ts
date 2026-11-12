// The models chosen for comparison ("Add to comparison", 0.0.5 item 4).
//
// WP0 stub with the final signatures: a working list of up to four entries. WP4b owns this
// file and fills in the rest (choosing the file of a repository entry, keeping the list
// while the app runs, anything the Compare tab needs). WP2 uses `useAddToComparison` on
// Installed rows.
import { create } from "zustand";
import type { CompareItem } from "../lib/types";

/** How many models can be compared at once. */
export const COMPARE_LIMIT = 4;

/**
 * What the list holds: a catalog entry, an installed file, or a file in a Hugging Face
 * repository. A repository entry may have no file yet (`file: null`, added from Browse
 * results); WP4b's rule picks one before the facts are asked for.
 */
export type CompareEntry = { catalogId: string } | { installedPath: string } | { repo: string; file: string | null };

/** Whether two entries name the same model (a repository entry by repository and file). */
export function sameEntry(a: CompareEntry, b: CompareEntry): boolean {
  if ("catalogId" in a) return "catalogId" in b && a.catalogId === b.catalogId;
  if ("installedPath" in a) return "installedPath" in b && a.installedPath === b.installedPath;
  return "repo" in b && a.repo === b.repo && a.file === b.file;
}

/** The backend's item for an entry; null for a repository entry whose file is not chosen yet. */
export function toCompareItem(entry: CompareEntry): CompareItem | null {
  if ("repo" in entry) return entry.file === null ? null : { repo: entry.repo, file: entry.file };
  return entry;
}

export interface CompareState {
  entries: CompareEntry[];
  /** Adds an entry. False when it is already in the list or the list is full. */
  add: (entry: CompareEntry) => boolean;
  remove: (entry: CompareEntry) => void;
  /** Sets the file of a repository entry ("Change" in the column). */
  setFile: (repo: string, from: string | null, to: string) => void;
  clear: () => void;
}

export const useCompare = create<CompareState>((set, get) => ({
  entries: [],
  add: (entry) => {
    const { entries } = get();
    if (entries.length >= COMPARE_LIMIT || entries.some((e) => sameEntry(e, entry))) return false;
    set({ entries: [...entries, entry] });
    return true;
  },
  remove: (entry) => set({ entries: get().entries.filter((e) => !sameEntry(e, entry)) }),
  setFile: (repo, from, to) =>
    set({
      entries: get().entries.map((e) => ("repo" in e && e.repo === repo && e.file === from ? { repo, file: to } : e)),
    }),
  clear: () => set({ entries: [] }),
}));

/** For an "Add to comparison" button: whether the entry is in the list, whether the list is full. */
export function useAddToComparison(entry: CompareEntry): {
  added: boolean;
  full: boolean;
  add: () => boolean;
  remove: () => void;
} {
  const entries = useCompare((s) => s.entries);
  const add = useCompare((s) => s.add);
  const remove = useCompare((s) => s.remove);
  const added = entries.some((e) => sameEntry(e, entry));
  return {
    added,
    full: !added && entries.length >= COMPARE_LIMIT,
    add: () => add(entry),
    remove: () => remove(entry),
  };
}
