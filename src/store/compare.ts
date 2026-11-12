// The models chosen for comparison ("Add to comparison", 0.0.5 item 4).
//
// Up to four entries, kept while the app runs (not saved). Browse results, repository pages
// and the Compare tab's suggestions add to it; WP2's Installed rows use
// `useAddToComparison`. The Compare tab (src/components/models/Compare.tsx) reads it.
//
// A repository entry names one file. Added from a repository page, it is the file the user
// picked there. Added from Browse results it has no file yet; the Compare tab then chooses
// one with `chooseFile` at the comparison size and records it with `autoFile`, and chooses
// again when the size changes, until the user picks one with Change (`setFile`).
import { create } from "zustand";
import type { CompareItem, RepoView, VariantView } from "../lib/types";

/** How many models can be compared at once. */
export const COMPARE_LIMIT = 4;

/** Shown on a disabled Add to comparison button and once on the page while the list is full. */
export const COMPARE_FULL = "You can compare up to 4 models. Remove one first.";

/**
 * What the list holds: a catalog entry, an installed file, or a file in a Hugging Face
 * repository. A repository entry may have no file yet (`file: null`, added from Browse
 * results); the Compare tab picks one before the facts are asked for.
 */
export type CompareEntry = { catalogId: string } | { installedPath: string } | { repo: string; file: string | null };

/** Whether two entries name the same model (a repository entry by repository and file). */
export function sameEntry(a: CompareEntry, b: CompareEntry): boolean {
  if ("catalogId" in a) return "catalogId" in b && a.catalogId === b.catalogId;
  if ("installedPath" in a) return "installedPath" in b && a.installedPath === b.installedPath;
  return "repo" in b && a.repo === b.repo && a.file === b.file;
}

/** A stable key for an entry (React keys and caches). */
export function entryKey(entry: CompareEntry): string {
  if ("catalogId" in entry) return `catalog\n${entry.catalogId}`;
  if ("installedPath" in entry) return `installed\n${entry.installedPath}`;
  return `repo\n${entry.repo}\n${entry.file ?? ""}`;
}

/** The backend's item for an entry; null for a repository entry whose file is not chosen yet. */
export function toCompareItem(entry: CompareEntry): CompareItem | null {
  if ("repo" in entry) return entry.file === null ? null : { repo: entry.repo, file: entry.file };
  if ("catalogId" in entry) return { catalogId: entry.catalogId };
  return { installedPath: entry.installedPath };
}

/**
 * The file a repository entry uses when the user has not picked one (0.0.5 item 4): the
 * largest file marked as fitting ("Fits", not "Fits with limits") at the comparison size,
 * else the smallest. A split model is named by its first part. Null without model files.
 */
export function chooseFile(variants: readonly VariantView[]): string | null {
  const withFile = variants.filter((v) => v.files.length > 0);
  const fitting = withFile.filter((v) => v.fit?.verdict === "green");
  const pick = fitting.length
    ? fitting.reduce((a, b) => (b.totalSize > a.totalSize ? b : a))
    : withFile.reduce<VariantView | null>((a, b) => (a === null || b.totalSize < a.totalSize ? b : a), null);
  return pick?.files[0]?.path ?? null;
}

export interface CompareState {
  entries: CompareEntry[];
  /** Adds an entry. False when it is already in the list or the list is full. */
  add: (entry: CompareEntry) => boolean;
  remove: (entry: CompareEntry) => void;
  /** Removes every entry of a repository (the Browse row's Remove from comparison). */
  removeRepo: (repo: string) => void;
  /**
   * Sets the file of a repository entry: the user's pick ("Change" in the column). When
   * that file is already in the list, the two entries become one.
   */
  setFile: (repo: string, from: string | null, to: string) => void;
  /** Sets the file the app chose for a repository entry (kept until the user picks one). */
  autoFile: (repo: string, from: string | null, to: string) => void;
  clear: () => void;
  /** Keys (`entryKey`) of repository entries whose file the app chose. */
  auto: string[];
  /** The comparison size ("Compare at …"), in tokens; null is the defaults' size. */
  ctx: number | null;
  setCtx: (ctx: number | null) => void;
  /**
   * Repository pages already fetched (on a repository page, or by the Compare tab), with
   * the size their fit marks were checked at, so the Change menu and the file rule need
   * no new request to Hugging Face.
   */
  repos: Record<string, { ctx: number | null; view: RepoView }>;
  rememberRepo: (view: RepoView, ctx: number | null) => void;
}

/** Whether the app (not the user) chose this repository entry's file. */
export function isAutoFile(state: Pick<CompareState, "auto">, entry: CompareEntry): boolean {
  return "repo" in entry && entry.file !== null && state.auto.includes(entryKey(entry));
}

export const useCompare = create<CompareState>((set, get) => {
  /** Moves a repository entry to another file, merging with an entry that has it. */
  function moveFile(repo: string, from: string | null, to: string, byApp: boolean) {
    const { entries, auto } = get();
    const fromEntry: CompareEntry = { repo, file: from };
    const toEntry: CompareEntry = { repo, file: to };
    if (!entries.some((e) => sameEntry(e, fromEntry))) return;
    const fromKey = entryKey(fromEntry);
    const toKey = entryKey(toEntry);
    const rest = auto.filter((k) => k !== fromKey);
    if (from !== to && entries.some((e) => sameEntry(e, toEntry))) {
      // Already in the list: keep that entry (and whether the app chose it).
      set({ entries: entries.filter((e) => !sameEntry(e, fromEntry)), auto: rest });
      return;
    }
    set({
      entries: entries.map((e) => (sameEntry(e, fromEntry) ? toEntry : e)),
      auto: byApp ? [...rest.filter((k) => k !== toKey), toKey] : rest.filter((k) => k !== toKey),
    });
  }

  return {
    entries: [],
    auto: [],
    ctx: null,
    repos: {},
    add: (entry) => {
      const { entries } = get();
      if (entries.length >= COMPARE_LIMIT || entries.some((e) => sameEntry(e, entry))) return false;
      set({ entries: [...entries, entry] });
      return true;
    },
    remove: (entry) =>
      set({
        entries: get().entries.filter((e) => !sameEntry(e, entry)),
        auto: get().auto.filter((k) => k !== entryKey(entry)),
      }),
    removeRepo: (repo) => {
      const gone = get().entries.filter((e) => "repo" in e && e.repo === repo).map(entryKey);
      set({
        entries: get().entries.filter((e) => !("repo" in e && e.repo === repo)),
        auto: get().auto.filter((k) => !gone.includes(k)),
      });
    },
    setFile: (repo, from, to) => moveFile(repo, from, to, false),
    autoFile: (repo, from, to) => moveFile(repo, from, to, true),
    clear: () => set({ entries: [], auto: [] }),
    setCtx: (ctx) => set({ ctx }),
    rememberRepo: (view, ctx) => set({ repos: { ...get().repos, [view.id]: { ctx, view } } }),
  };
});

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

/**
 * For a Browse result: added when any file of the repository is in the list. Adding puts
 * the repository in without a file (the Compare tab chooses one); removing takes out every
 * file of it.
 */
export function useRepoInComparison(repo: string): {
  added: boolean;
  full: boolean;
  add: () => boolean;
  remove: () => void;
} {
  const entries = useCompare((s) => s.entries);
  const add = useCompare((s) => s.add);
  const removeRepo = useCompare((s) => s.removeRepo);
  const added = entries.some((e) => "repo" in e && e.repo === repo);
  return {
    added,
    full: !added && entries.length >= COMPARE_LIMIT,
    add: () => add({ repo, file: null }),
    remove: () => removeRepo(repo),
  };
}
