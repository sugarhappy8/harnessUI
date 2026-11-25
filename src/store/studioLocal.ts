// State for Studio model discovery, downloads, repairs and the one-time local model notice.
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import type { StoreApi } from "zustand";
import * as api from "../components/studio/api";
import type {
  ImportInspection,
  ImportMode,
  ImportProgress,
  ImportResult,
  LocalRepo,
  LocalSearchHit,
  LocalSource,
  Lora,
  ModelFiles,
  SearchFilter,
  SetModelFilesResult,
  SetupChoice,
  SetupPlan,
} from "../components/studio/types";
import type { ErrorInfo } from "../lib/types";
import { IMPORT } from "../lib/words";
import { toErrorInfo } from "./app";
import { useDownloads } from "./downloads";
import type { StudioState, StudioTab } from "./studio";

/** The part of Studio's state this module keeps. */
export interface StudioLocalState {
  /** Get models, Studio's full view, in place of the results, the composer and the side panel. */
  localView: boolean;
  /** Get models in place of the results (its filter starts on `tab`'s kind), or the results
   *  again (`tab` stays as it was). */
  showLocalModels(show: boolean): void;
  /** Get models' filter, Pictures, Video or LoRAs: set from `tab` each time the view opens;
   *  changing it leaves `tab` as it is. */
  searchFilter: SearchFilter;
  setSearchFilter(filter: SearchFilter): void;
  /** *Fits this computer* (off by default, coordinator ruling P35): on, the catalog models that
   *  may not fit, and the opened results with no file for this computer, are hidden. */
  fitsOnly: boolean;
  setFitsOnly(on: boolean): void;
  /** The last Hugging Face search: its words, the filter it was for and what it found; `null`
   *  before the first. */
  localSearch: { text: string; filter: SearchFilter; hits: LocalSearchHit[] } | null;
  /** Search or Enter in Get models (never as typing goes), and its Popular list as it opens:
   *  asks Hugging Face for
   *  `filter`'s kind of repositories matching `text`; empty `text` lists the most downloaded
   *  (Popular). An answer to a search asked before the last one is dropped. Rejects when
   *  Hugging Face could not be asked. */
  search(text: string, filter: SearchFilter): Promise<void>;
  /** One search result opened: asks Hugging Face for repository `repo` at its latest commit,
   *  as Studio recognises it, keeps it in `repos`, and makes its hit's verdict exact in
   *  `localSearch`. */
  openRepo(repo: string): Promise<LocalRepo>;
  /** The repositories opened this session, by name: the Fits switch and *Use it* read them. */
  repos: Record<string, LocalRepo>;
  /** A file's set-up with the recipe or files `choice` names. */
  setupPlan(source: LocalSource, choice: SetupChoice): Promise<SetupPlan>;
  /** Downloads `source` set up as `choice` (`null`: as its plan comes), then reads the
   *  downloads and the local models again. Returns the download group's ID. */
  downloadSetup(source: LocalSource, choice: SetupChoice | null): Promise<string>;
  /** *Add a model file* / *Add a LoRA file*: the file chooser (.safetensors, .sft, .gguf);
   *  `null` when it was closed without a file. */
  chooseFile(kind: "model" | "lora"): Promise<string | null>;
  /** What the file at `path` is, from its header. Sends nothing. */
  importInspect(path: string): Promise<ImportInspection>;
  /** *Add*: the file at `path` added as `mode` says, set up as `choice` (`null`: as its plan
   *  comes), then the local models and downloads are read again. `importProgress` follows it. */
  importModel(path: string, mode: ImportMode, choice: SetupChoice | null): Promise<ImportResult>;
  /** How far the import being made is; `null` while none is. */
  importProgress: ImportProgress | null;
  /** An installed model's files (*Set it up yourself*). */
  modelFiles(genModelId: string): Promise<ModelFiles>;
  /** Sets an installed model up with `choice`, then reads the local models and downloads
   *  again. */
  setModelFiles(genModelId: string, choice: SetupChoice): Promise<SetModelFilesResult>;
  /** The LoRAs, newest first; read by `loadLoras` and kept up by the actions below (which keep
   *  `local.loras`, Get models' list, up too). */
  loras: Lora[];
  loadLoras(): Promise<void>;
  /** Downloads a searched LoRA file; returns the download group's ID (the LoRA joins `loras`
   *  once its file is checked: `loadLoras` again). */
  downloadLora(repo: string, commit: string, file: string): Promise<string>;
  /** *Add a LoRA file*: copied into Studio's LoRA folder, with `importProgress`. */
  importLora(path: string): Promise<Lora>;
  /** Deletes a LoRA and its file; rejects while a run on this computer uses it. */
  deleteLora(id: string): Promise<void>;
  /** Delete on an installed picture or video model: its files go (not a part another model
   *  uses), its row leaves the list, then the local models are read again (a read that fails
   *  only leaves a trace: the files are gone all the same). Rejects when it was refused (while
   *  a run on this computer uses the model, `studio_model_in_use`). */
  deleteLocal(genModelId: string): Promise<void>;
  /** Checks the installed model and starts a download of any missing or damaged files. */
  repairLocal(genModelId: string): Promise<void>;
  /** The model Get models' detail panel shows (Task U6): a Hugging Face result by its repository,
   *  or a catalog model by its entry; `null` while the panel is closed. */
  detail: { repo: string } | { entryId: string } | null;
  /** Why the repository the panel shows could not be read; `null` otherwise. */
  detailError: ErrorInfo | null;
  /** Opens the detail panel on `d`. A repository is read with `openRepo` (what was read of it
   *  this session shows while it is read again); a read that fails is kept in `detailError`
   *  for the panel to say, so this never rejects. */
  openDetail(d: { repo: string } | { entryId: string }): Promise<void>;
  closeDetail(): void;
  /** *I understand* on the note about local models: the backend records it (local downloads
   *  are refused until then), and the local models and preferences say so. */
  acceptNotice(): Promise<void>;
  /** The note about local models read again, read only, from Get models' footer
   *  (`"read"`); `null` while it is closed. (Before a first download the detail panel shows it
   *  itself, to accept.) */
  noticeOpen: "read" | null;
  setNoticeOpen(mode: "read" | null): void;
}

/** The filter Get models opens on, from the tab shown. */
const FILTER_OF_TAB: Record<StudioTab, SearchFilter> = { image: "pictures", video: "video" };

/** Counts Hugging Face searches as they are asked, so only the last one's answer is kept. */
let searchTick = 0;

/** What a file chooser offers for each kind of file. */
const CHOOSER = {
  model: IMPORT.addModel,
  lora: IMPORT.addLora,
} as const;

/** Reads the downloads and the local models again after one was started or set up; a read that
 *  fails only leaves a trace (the download started all the same). */
async function refreshAfterDownload(get: StoreApi<StudioState>["getState"]): Promise<void> {
  await useDownloads
    .getState()
    .read()
    .catch((e: unknown) => console.warn("Studio: the downloads could not be read", e));
  await get()
    .refreshLocal()
    .catch((e: unknown) => console.warn("Studio: the local models could not be read again", e));
}

/** Runs `import` while `importProgress` follows `studio-import-progress`; it is cleared when
 *  the import ends, however it ends, and an event heard later is dropped. */
async function followingImport<T>(set: StoreApi<StudioState>["setState"], run: () => Promise<T>): Promise<T> {
  let active = true;
  let stop: (() => void) | undefined;
  try {
    stop = await api.onImportProgress((importProgress) => {
      if (active) set({ importProgress });
    });
  } catch (e) {
    // The import goes on without a bar.
    console.warn("Studio: import progress could not be followed", e);
  }
  try {
    return await run();
  } finally {
    active = false;
    stop?.();
    set({ importProgress: null });
  }
}

/** Get models' state and actions, for `useStudio`. */
export function createLocal(
  set: StoreApi<StudioState>["setState"],
  get: StoreApi<StudioState>["getState"],
): StudioLocalState {
  return {
    localView: false,
    // The detail panel belongs to the view: it never outlives it, nor shows again as it opens.
    showLocalModels: (show) =>
      set((s) =>
        show
          ? { localView: true, searchFilter: FILTER_OF_TAB[s.tab], detail: null, detailError: null }
          : { localView: false, detail: null, detailError: null },
      ),
    searchFilter: "pictures",
    setSearchFilter: (searchFilter) => set({ searchFilter }),
    fitsOnly: false,
    setFitsOnly: (fitsOnly) => set({ fitsOnly }),
    localSearch: null,
    search: async (text, filter) => {
      const mine = ++searchTick;
      const page = await api.studioLocalSearch(text, filter);
      if (mine === searchTick) set({ localSearch: { text, filter, hits: page.items } });
    },
    repos: {},
    openRepo: async (repo) => {
      const opened = await api.studioLocalRepo(repo);
      set((s) => ({
        repos: { ...s.repos, [repo]: opened },
        // Its files were read: what the hit says it is now is exact.
        localSearch: s.localSearch
          ? {
              ...s.localSearch,
              hits: s.localSearch.hits.map((hit) =>
                hit.repo === repo ? { ...hit, verdict: opened.verdict, exact: true } : hit,
              ),
            }
          : null,
      }));
      return opened;
    },
    setupPlan: (source, choice) => api.studioSetupPlan(source, choice),
    downloadSetup: async (source, choice) => {
      const group = await api.studioLocalDownload(source, choice);
      await refreshAfterDownload(get);
      return group;
    },
    chooseFile: async (kind) => {
      const chosen = await openDialog({
        multiple: false,
        directory: false,
        filters: [{ name: CHOOSER[kind], extensions: ["safetensors", "sft", "gguf"] }],
      });
      return typeof chosen === "string" ? chosen : null;
    },
    importInspect: (path) => api.studioImportInspect(path),
    importModel: async (path, mode, choice) => {
      const result = await followingImport(set, () => api.studioImportModel(path, mode, choice));
      await refreshAfterDownload(get);
      return result;
    },
    importProgress: null,
    modelFiles: (genModelId) => api.studioModelFiles(genModelId),
    setModelFiles: async (genModelId, choice) => {
      const result = await api.studioSetModelFiles(genModelId, choice);
      await refreshAfterDownload(get);
      return result;
    },
    loras: [],
    loadLoras: async () => {
      set({ loras: await api.studioLoras() });
    },
    downloadLora: async (repo, commit, file) => {
      const group = await api.studioLoraDownload(repo, commit, file);
      await useDownloads
        .getState()
        .read()
        .catch((e: unknown) => console.warn("Studio: the downloads could not be read", e));
      return group;
    },
    importLora: async (path) => {
      const lora = await followingImport(set, () => api.studioLoraImport(path));
      // The same file added again is the same LoRA: it keeps its place. Get models lists the
      // local models' own copy, so it gains it too.
      const add = (list: Lora[]) => [lora, ...list.filter((l) => l.id !== lora.id)];
      set((s) => ({ loras: add(s.loras), local: s.local ? { ...s.local, loras: add(s.local.loras) } : null }));
      return lora;
    },
    deleteLora: async (id) => {
      await api.studioLoraDelete(id);
      const drop = (list: Lora[]) => list.filter((l) => l.id !== id);
      set((s) => ({ loras: drop(s.loras), local: s.local ? { ...s.local, loras: drop(s.local.loras) } : null }));
    },
    deleteLocal: async (genModelId) => {
      await api.studioLocalDelete(genModelId);
      // Its files are gone: its row goes now, whether or not the read after it works.
      set((s) =>
        s.local ? { local: { ...s.local, installed: s.local.installed.filter((m) => m.id !== genModelId) } } : {},
      );
      await get()
        .refreshLocal()
        .catch((e: unknown) => console.warn("Studio: the local models could not be read again", e));
    },
    repairLocal: async (genModelId) => {
      await api.studioLocalDownload({ installed: { genModelId } }, null);
    },
    detail: null,
    detailError: null,
    openDetail: async (d) => {
      set({ detail: d, detailError: null });
      if (!("repo" in d)) return;
      try {
        await get().openRepo(d.repo);
      } catch (e) {
        // Said in the panel, while it still shows this repository.
        set((s) => (s.detail === d ? { detailError: toErrorInfo(e) } : {}));
      }
    },
    closeDetail: () => set({ detail: null, detailError: null }),
    acceptNotice: async () => {
      const prefs = await api.studioAcceptLocalNotice();
      set((s) => ({ prefs, local: s.local ? { ...s.local, noticeAccepted: true } : null }));
    },
    noticeOpen: null,
    setNoticeOpen: (noticeOpen) => set({ noticeOpen }),
  };
}
