// Studio's store (0.1.0): which view the main window shows, the chat or Studio (the chat
// stays mounted, and its reply keeps running, while Studio is shown), Studio's tab, and the
// runs as the backend reports them (`studio-progress`; `studio_live` once at start, as events
// are not replayed). A video that finishes while the chat is shown raises a notice there.
// Task B2: what Studio has saved (endpoints and their lists, local models, the gallery's first
// page, preferences, today's spending), read once when Studio opens and never fetched then;
// each tab's draft, which starts at the model used last or, local first, a default.
import { create, type StoreApi, type UseBoundStore } from "zustand";
import * as api from "../components/studio/api";
import { defaultsFor } from "../components/studio/canonical";
import type {
  Company,
  InstalledLocal,
  LocalEntry,
  LocalModels,
  MediaKind,
  MediaModel,
  RefItem,
  Routing,
  Run,
  RunsCursor,
  RunsFilter,
  RunStatus,
  RunTarget,
  StudioEndpoint,
  StudioPrefs,
  StudioPrefsPatch,
  Usage,
} from "../components/studio/types";
import * as ipc from "../lib/ipc";
import { listenInEffect } from "../lib/listen";

/** Studio's tabs: Pictures and Video. */
export type StudioTab = MediaKind;

/** A tab's draft: the model it is set to and what Create would send. */
export interface Draft {
  /** Where a run goes; `null` until there is a model to use. */
  target: RunTarget | null;
  model: string | null;
  prompt: string;
  /** The model's settings by field key; an unset field has no key. */
  values: Record<string, unknown>;
  /** Extra fields (JSON), for a company that takes them. */
  extra: Record<string, unknown>;
  refs: RefItem[];
  /** The seed stays as it is for the next run. */
  seedLocked: boolean;
  /** Which companies may run an OpenRouter picture. */
  routing: Routing | null;
  /** What changed when the model was switched, until it is put away. */
  note: string | null;
}

/** Which empty state a tab shows in place of the card stream, if any. */
export type EmptyState = "getLocal" | "cloudBackup" | "addEndpoint" | null;

export interface StudioState {
  /** What the main window shows beside the chat list. */
  view: "chat" | "studio";
  tab: StudioTab;
  /** Each run the backend reported, by id, at its newest state. */
  live: Record<string, Run>;
  /** The id of a video that finished while the chat was shown. */
  readyNotice: string | null;
  /** Opens Studio, on `tab` when given (else on the tab shown last). */
  showStudio(tab?: StudioTab): void;
  showChat(): void;
  /** A run's new state, from `studio-progress` or `studio_live`. */
  applyProgress(run: Run): void;

  /** The endpoints that make pictures or video, with their saved lists. */
  endpoints: StudioEndpoint[];
  /** What this computer can run; `null` until read. */
  local: LocalModels | null;
  /** Everything saved has been read once. */
  loaded: boolean;
  drafts: Record<StudioTab, Draft>;
  /** The gallery's runs read so far, newest first. */
  runs: Run[];
  /** Where the next page of runs starts; `null` after the last page. */
  next: RunsCursor | null;
  filter: RunsFilter;
  prefs: StudioPrefs | null;
  usage: Usage | null;
  /** The gallery's grid in place of the card stream. */
  grid: boolean;
  /** Get local models in place of the card stream. */
  localView: boolean;
  /** Reads what is saved: the lists, local models, the gallery's first page, the runs still
   *  working, preferences and today's spending. Never fetches a list or a price; asked again
   *  while reading, it gives the same read. Each tab without a model then gets its first. */
  load(): Promise<void>;
  /** *Get the list*, *Update model lists*: fetches endpoint `providerId`'s lists. */
  refreshModels(providerId: number): Promise<void>;
  /** Reads the local models again (after a download or a delete). */
  refreshLocal(): Promise<void>;
  setTab(tab: StudioTab): void;
  showLocalModels(show: boolean): void;
  /** Sets `tab`'s draft to the model used last when it is still offered, else to
   *  `defaultModel`, at that model's starting values. Fetches nothing (not even a price). */
  selectInitialModel(tab: StudioTab): void;
  setPrompt(tab: StudioTab, text: string): void;
  /** Changes the gallery's filter and reads its first page again. */
  setFilter(patch: Partial<RunsFilter>): Promise<void>;
  setGrid(grid: boolean): void;
  /** Changes only the preferences `patch` names. */
  setPrefs(patch: StudioPrefsPatch): Promise<void>;
}

const EMPTY_DRAFT: Draft = {
  target: null,
  model: null,
  prompt: "",
  values: {},
  extra: {},
  refs: [],
  seedLocked: false,
  routing: null,
  note: null,
};

/** How many runs one page of the gallery holds. */
const RUNS_PAGE = 50;

/** The read of what is saved while it is under way, so a second `load` waits for it. */
let loading: Promise<void> | null = null;

export const useStudio: UseBoundStore<StoreApi<StudioState>> = create<StudioState>((set, get) => {
  /** Gives each tab still without a model its first one (a list or a model got later). */
  const fillEmptyDrafts = () => {
    for (const tab of ["image", "video"] as const) {
      if (get().drafts[tab].model === null) get().selectInitialModel(tab);
    }
  };

  return {
    view: "chat",
    tab: "image",
    live: {},
    readyNotice: null,
    showStudio: (tab) => set((s) => ({ view: "studio", tab: tab ?? s.tab })),
    showChat: () => set({ view: "chat" }),
    applyProgress: (run) =>
      set((s) => {
        const finished = run.kind === "video" && run.status === "done" && s.live[run.id]?.status !== "done";
        return {
          live: { ...s.live, [run.id]: run },
          readyNotice: finished && s.view === "chat" ? run.id : s.readyNotice,
        };
      }),

    endpoints: [],
    local: null,
    loaded: false,
    drafts: { image: EMPTY_DRAFT, video: EMPTY_DRAFT },
    runs: [],
    next: null,
    filter: {},
    prefs: null,
    usage: null,
    grid: false,
    localView: false,
    load: () => {
      loading ??= (async () => {
        try {
          const [models, local, page, live, prefs, usage] = await Promise.all([
            api.studioModels(),
            api.studioLocalModels(),
            api.studioRuns(get().filter, null, RUNS_PAGE),
            api.studioLive(),
            api.studioGetPrefs(),
            api.studioUsage(),
          ]);
          set({ endpoints: models.endpoints, local, runs: page.runs, next: page.next, prefs, usage, loaded: true });
          for (const run of live) get().applyProgress(run);
          fillEmptyDrafts();
        } finally {
          loading = null;
        }
      })();
      return loading;
    },
    refreshModels: async (providerId) => {
      const { endpoints } = await api.studioRefreshModels(providerId);
      set({ endpoints });
      fillEmptyDrafts();
    },
    refreshLocal: async () => {
      const local = await api.studioLocalModels();
      set({ local });
      fillEmptyDrafts();
    },
    setTab: (tab) => set({ tab }),
    showLocalModels: (show) => set({ localView: show }),
    selectInitialModel: (tab) =>
      set((s) => {
        const offered = (c: { target: RunTarget; model: string }) =>
          offeredModel(tab, c.target, c.model, s.local, s.endpoints);
        const last = s.prefs?.lastModel[tab];
        const choice = last && offered(last) ? last : defaultModel(tab, s.local, s.endpoints);
        const model = choice ? offered(choice) : null;
        const draft: Draft = {
          ...EMPTY_DRAFT,
          prompt: s.drafts[tab].prompt,
          target: choice?.target ?? null,
          model: choice?.model ?? null,
          values: model ? defaultsFor(model) : {},
        };
        return { drafts: { ...s.drafts, [tab]: draft } };
      }),
    setPrompt: (tab, text) => set((s) => ({ drafts: { ...s.drafts, [tab]: { ...s.drafts[tab], prompt: text } } })),
    setFilter: async (patch) => {
      const filter = { ...get().filter, ...patch };
      set({ filter });
      const page = await api.studioRuns(filter, null, RUNS_PAGE);
      // An answer for a filter changed since is old.
      if (get().filter === filter) set({ runs: page.runs, next: page.next });
    },
    setGrid: (grid) => set({ grid }),
    setPrefs: async (patch) => {
      const prefs = await api.studioSetPrefs(patch);
      set({ prefs });
    },
  };
});

// ---- the models a tab can use, and its empty state ----------------------------------------

/** Which kinds each company makes (OpenAI's video API shut down in September 2026). */
const MAKES: Record<Company, readonly MediaKind[]> = {
  openrouter: ["image", "video"],
  openai: ["image"],
  local: ["image", "video"],
};

/** The fit words Studio counts as fitting: fits, and fits after the chat model is unloaded. */
const FITTING: ReadonlySet<string> = new Set(["fits", "afterUnload"]);

/** The cloud models Studio picks first, when the endpoint offers them. */
const PREFERRED: Record<MediaKind, readonly string[]> = {
  image: ["gpt-image-2.6-flare", "openai/gpt-image-2.6-flare"],
  video: ["google/veo-3.1-fast"],
};

/** `local` when this computer can make `tab`'s kind at all (video needs a usable graphics
 *  card or an Apple-silicon Mac, and is hidden otherwise); `null` too until it is read. */
function localFor(tab: StudioTab, local: LocalModels | null): LocalModels | null {
  return local && (tab === "image" || local.videoPossible) ? local : null;
}

/** The installed local models `tab` can use: this version can run them, and this computer
 *  can make the kind. */
function localModelsFor(tab: StudioTab, local: LocalModels | null): InstalledLocal[] {
  return localFor(tab, local)?.installed.filter((m) => m.kind === tab && m.model.usable === null) ?? [];
}

/** The models of `endpoint`'s saved lists that `tab` can use. */
function cloudModelsFor(endpoint: StudioEndpoint, tab: StudioTab): MediaModel[] {
  return endpoint.models.filter((m) => m.kind === tab && m.usable === null);
}

/** The first recommended entry of `tab`'s kind that fits this computer. */
export function fittingEntry(tab: StudioTab, local: LocalModels | null): LocalEntry | null {
  return localFor(tab, local)?.recommended.find((e) => e.kind === tab && FITTING.has(e.fit.word)) ?? null;
}

/** Whether `endpoint`'s company makes `tab`'s kind, with or without its lists saved. */
export function makesKind(endpoint: StudioEndpoint, tab: StudioTab): boolean {
  return MAKES[endpoint.company].includes(tab);
}

/** Whether no local model this version can run is installed (of any kind): the first-run
 *  suggestion shows only then. */
export function nothingInstalled(local: LocalModels | null): boolean {
  return !local?.installed.some((m) => m.model.usable === null);
}

/** The model `id` at `target`, when `tab` can still use it. */
function offeredModel(
  tab: StudioTab,
  target: RunTarget,
  id: string,
  local: LocalModels | null,
  endpoints: StudioEndpoint[],
): MediaModel | null {
  if (target === "thisComputer") return localModelsFor(tab, local).find((m) => m.model.id === id)?.model ?? null;
  const endpoint = endpoints.find((e) => e.providerId === target.endpoint);
  return (endpoint && cloudModelsFor(endpoint, tab).find((m) => m.id === id)) ?? null;
}

/** The model `tab` starts with when none was used last: an installed local model that fits
 *  (not one that is slow here), else the first usable model of the first endpoint with one,
 *  its own GPT Image 2.6 Flare or Veo 3.1 Fast when offered; `null` when there is none. Never
 *  an "Auto" model, which would hide where the prompt goes. */
export function defaultModel(
  tab: StudioTab,
  local: LocalModels | null,
  endpoints: StudioEndpoint[],
): { target: RunTarget; model: string } | null {
  const installed = localModelsFor(tab, local).find((m) => FITTING.has(m.fit.word));
  if (installed) return { target: "thisComputer", model: installed.model.id };
  for (const endpoint of endpoints) {
    const models = cloudModelsFor(endpoint, tab);
    const preferred = PREFERRED[tab].map((id) => models.find((m) => m.id === id)).find((m) => m !== undefined);
    const model = preferred ?? models[0];
    if (model) return { target: { endpoint: endpoint.providerId }, model: model.id };
  }
  return null;
}

/**
 * What `tab` shows in place of the card stream, by one ordered rule, the first that applies:
 * nothing when the tab has a model to use (an installed local model of its kind, or an
 * endpoint's); *Get local models* when a recommended entry of its kind fits this computer
 * (after the chat model is unloaded counts; video also needs a computer that can make it);
 * cloud as the backup when no endpoint is saved; *add an endpoint* when the saved ones make
 * no such kind (the Video tab with only OpenAI). An endpoint that makes the kind but has no
 * saved list shows nothing here: *Get the list* stands in for it.
 */
export function emptyState(tab: StudioTab, local: LocalModels | null, endpoints: StudioEndpoint[]): EmptyState {
  if (localModelsFor(tab, local).length > 0 || endpoints.some((e) => cloudModelsFor(e, tab).length > 0)) return null;
  if (fittingEntry(tab, local)) return "getLocal";
  if (endpoints.length === 0) return "cloudBackup";
  if (!endpoints.some((e) => makesKind(e, tab))) return "addEndpoint";
  return null;
}

/** The states in which a run works whether or not Studio watches it. */
const WORKING: ReadonlySet<RunStatus> = new Set<RunStatus>(["queued", "sending", "running", "downloading"]);

/** The runs working now: queued, sending, running, downloading, and waiting while watched
 *  (a video no longer checked is not). */
export function runningCount(s: StudioState): number {
  return Object.values(s.live).filter((r) => WORKING.has(r.status) || (r.status === "waiting" && r.watching))
    .length;
}

/** Follows the runs: each `studio-progress` and, once listening has started, the runs still
 *  working now (a window that reloaded heard nothing of them). What an event says is newer
 *  than that answer. Returns the function that stops following; outside the app it follows
 *  nothing. */
export function followStudio(): () => void {
  if (!ipc.isTauri()) return () => undefined;
  const heard = new Set<string>();
  let stopped = false;
  const stopListening = listenInEffect(async () => {
    const unlisten = await api.onStudioProgress((run) => {
      if (stopped) return;
      heard.add(run.id);
      useStudio.getState().applyProgress(run);
    });
    if (!stopped) {
      Promise.resolve()
        .then(() => api.studioLive())
        .then((runs) => {
          if (stopped) return;
          for (const run of runs) if (!heard.has(run.id)) useStudio.getState().applyProgress(run);
        })
        .catch(() => undefined);
    }
    return unlisten;
  });
  return () => {
    stopped = true;
    stopListening();
  };
}
