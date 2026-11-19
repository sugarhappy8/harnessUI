// Studio's actions on a result (0.1.0 Task B9), composed into `useStudio` (src/store/studio.ts,
// which re-exports what other modules use from here; import it from there): Reuse all and
// Reuse without seed, which fill the prompt box and never start a run; Animate, Edit and Use as
// reference, which keep the picture as an added picture (another model offered when the tab's
// cannot take it, an installed local one first); Star; Delete, after which a finished run left
// without results goes too; several saved into one folder; and Settings → Studio's Delete
// stopped and failed runs (Task B10).
import type { StoreApi } from "zustand";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import * as api from "../components/studio/api";
import type { MediaModel, ModelChoice, Output, RefItem, RefRole, Run, RunStatus } from "../components/studio/types";
import { STUDIO_OFFER, STUDIO_REUSE } from "../lib/words";
import {
  cloudModelsFor,
  draftModel,
  FITTING,
  listedModel,
  localModelsFor,
  offeredModel,
  pictureRoles,
  RECENT_RUNS,
  recentChoices,
  type GoneWords,
  type SavedModel,
  type StudioState,
  type StudioTab,
} from "./studio";
import { seedKey, targetOf } from "./studioRun";

/** A model offered for Animate or Edit when the tab's model cannot take the picture. */
export interface ModelOffer {
  /** The question ("No Frames takes no start frame. Use Grok Imagine Video 1.5 Lite
   *  instead?"), or why there is nothing to offer. */
  text: string;
  /** The model offered and where it runs; `null` when no model here can take the picture. */
  choice: ModelChoice | null;
  /** The offered model's name, for its button; `null` with no choice. */
  name: string | null;
}

/** How a run ends without finishing: what *Delete stopped and failed runs* deletes (the
 *  backend's `unfinished_creation_run_ids`). */
const UNFINISHED: ReadonlySet<RunStatus> = new Set<RunStatus>(["failed", "blocked", "cancelled", "interrupted"]);

/** The part of Studio's state this module keeps. */
export interface StudioResultsState {
  /** Reuse all, or Reuse without seed (`withSeed` false: the seed goes back to Random): the tab
   *  of `run`'s kind gets its prompt, its model where it ran, every setting, its Extra fields,
   *  routing and added pictures (up to the model's limit), and is shown. Never starts a run.
   *  When the model is no longer offered there, its family's newest takes its place, the
   *  settings carried to it (`carry`) with a note of what changed. Returns why it cannot be
   *  reused (its model and family are gone, or its endpoint or key), or `null`. */
  reuse(run: Run, withSeed: boolean): string | null;
  /** Animate: picture `output` as the start frame of the Video tab's draft (in place of one
   *  there), on its model or, given `use`, on that model (picked as in the picker); the Video tab
   *  is shown. While the draft's model takes no start frame and no `use` is given, nothing
   *  changes and the model to offer comes back: an installed local one first, then the one used
   *  most recently, then the first an endpoint lists. Rejects when the picture could not be kept
   *  as an added picture. */
  animate(output: Output, use?: ModelChoice): Promise<ModelOffer | null>;
  /** Edit (with words): picture `output` as the picture to change, the one added picture of the
   *  Pictures tab's draft, on its model or, given `use`, on that model; the Pictures tab is
   *  shown and the prompt gets the cursor (`app:focus-studio-prompt`). While the draft's model
   *  takes no pictures and no `use` is given, nothing changes and the model to offer comes back,
   *  as for `animate`. Rejects when the picture could not be kept. */
  editWithWords(output: Output, use?: ModelChoice): Promise<ModelOffer | null>;
  /** Use as reference: picture `output` added to the shown tab's draft, as the first of its
   *  model's picture roles with room. Returns why it was not added (the model takes no more), or
   *  `null`; rejects when the picture could not be kept. */
  useAsReference(output: Output): Promise<string | null>;
  /** Stars or unstars result `outputId`; its new state joins the runs. */
  star(outputId: string, starred: boolean): Promise<void>;
  /** Deletes results `outputIds`, their files and records (today's spending stays as it is):
   *  they leave the runs, and a finished run left without results goes with them. */
  remove(outputIds: string[]): Promise<void>;
  /** *Delete stopped and failed runs*: every run that ended without finishing (failed, blocked,
   *  cancelled or interrupted) is deleted with its files, as the backend counts them, and leaves
   *  the runs. Resolves with how many went. */
  removeUnfinished(): Promise<number>;
  /** Save several: the folder picker, then each result copied into that folder under its own
   *  file name, without its settings. Resolves with the folder, or `null` (nothing saved) when
   *  the picker is cancelled; a copy that fails does not stop the others, and the first failure
   *  rejects once all were tried. */
  saveSeveral(outputIds: string[]): Promise<string | null>;
}

/** What the result actions use of the store's own workings. */
export interface ResultsDeps {
  /** Puts `saved` (a run to reuse) in `tab`'s draft with `refs` and `prompt`, its model's
   *  family's newest when the model is gone (`words` say so); returns why it cannot, or `null`. */
  fillDraft(tab: StudioTab, saved: SavedModel, refs: RefItem[], prompt: string, words: GoneWords): string | null;
  /** Reads today's spending again (the top bar and the daily limit follow it). */
  rereadUsage(): void;
}

/** Studio's actions on a result, on the store `set` and `get` work on. Builds nothing but the
 *  closures, so it can run while the store is being made. */
export function createResults(
  set: StoreApi<StudioState>["setState"],
  get: StoreApi<StudioState>["getState"],
  deps: ResultsDeps,
): StudioResultsState {
  /** Animate's and Edit's part: picture `output` in `tab`'s draft as `role` (Animate's start
   *  frame takes the place of one there; Edit's picture is the only one), on the draft's model or
   *  on `use`; `tab` is shown. While the draft's model takes no such picture and no `use` is
   *  given, the model to offer instead (`offer` words the question). */
  const putPicture = async (
    tab: StudioTab,
    role: RefRole,
    output: Output,
    use: ModelChoice | undefined,
    offer: (model: string | null, offered: string) => string,
    none: string,
  ): Promise<ModelOffer | null> => {
    const s = get();
    const current = draftModel(s, tab);
    if (!use && !(current && takesRole(current, role))) {
      const found = modelTaking(s, tab, role);
      if (!found) return { text: none, choice: null, name: null };
      const name = found.model.displayName;
      return { text: offer(current?.displayName ?? null, name), choice: found.choice, name };
    }
    const kept = await api.studioRefFromOutput(output.id);
    if (use) get().chooseModel(tab, use.target, use.model);
    const ref: RefItem = { ...kept, role };
    set((st) => {
      const draft = st.drafts[tab];
      const refs = role === "startFrame" ? [ref, ...draft.refs.filter((r) => r.role !== "startFrame")] : [ref];
      return { tab, drafts: { ...st.drafts, [tab]: { ...draft, refs } } };
    });
    return null;
  };

  return {
    reuse: (run, withSeed) => {
      const target = targetOf(run);
      if (target === null) return STUDIO_REUSE.endpointGone;
      const s = get();
      // Without its seed, the seed goes back to Random: Studio picks a new one for the run.
      const seed = withSeed ? null : seedKey(listedModel(target, run.model, s.local, s.endpoints));
      const params = Object.fromEntries(Object.entries(run.params).filter(([key]) => key !== seed));
      const refs = run.refs.map((r) => ({ ...r, role: r.role ?? "reference" }));
      const saved = { target, model: run.model, params, extra: run.extra, routing: run.routing };
      const refused = deps.fillDraft(run.kind, saved, refs, run.prompt, STUDIO_REUSE);
      if (refused === null) set({ tab: run.kind });
      return refused;
    },
    animate: (output, use) =>
      putPicture("video", "startFrame", output, use, STUDIO_OFFER.animate, STUDIO_OFFER.noneAnimate),
    editWithWords: async (output, use) => {
      const offer = await putPicture("image", "reference", output, use, STUDIO_OFFER.edit, STUDIO_OFFER.noneEdit);
      if (offer === null) window.dispatchEvent(new Event("app:focus-studio-prompt"));
      return offer;
    },
    useAsReference: async (output) => {
      const kept = await api.studioRefFromOutput(output.id);
      return get().addRef(get().tab, { ...kept, role: null });
    },
    star: async (outputId, starred) => {
      const output = await api.studioStar(outputId, starred);
      const change = (run: Run): Run =>
        run.id === output.runId ? { ...run, outputs: run.outputs.map((o) => (o.id === output.id ? output : o)) } : run;
      set((s) => ({
        runs: s.runs.map(change),
        recentRuns: s.recentRuns.map(change),
        live: Object.fromEntries(Object.entries(s.live).map(([id, run]) => [id, change(run)])),
      }));
    },
    remove: async (outputIds) => {
      await api.studioDelete(outputIds);
      const gone = new Set(outputIds);
      // As the backend: a finished run left without results goes; one still working, or one that
      // ended without finishing, keeps its card.
      const strip = (run: Run): Run | null => {
        const outputs = run.outputs.filter((o) => !gone.has(o.id));
        if (outputs.length === run.outputs.length) return run;
        return run.status === "done" && outputs.length === 0 ? null : { ...run, outputs };
      };
      const kept = (runs: Run[]) => runs.map(strip).filter((r): r is Run => r !== null);
      set((s) => ({
        runs: kept(s.runs),
        recentRuns: kept(s.recentRuns),
        live: Object.fromEntries(
          Object.entries(s.live).flatMap(([id, run]) => {
            const left = strip(run);
            return left ? [[id, left]] : [];
          }),
        ),
      }));
      // What the gallery holds changed (today's spending does not).
      deps.rereadUsage();
    },
    removeUnfinished: async () => {
      const count = await api.studioDeleteUnfinished();
      set((s) => {
        // Each run as it stands now: one reported since its page was read by that report.
        const unfinished = (run: Run) => UNFINISHED.has((s.live[run.id] ?? run).status);
        const kept = (runs: Run[]) => runs.filter((run) => !unfinished(run));
        return {
          runs: kept(s.runs),
          recentRuns: kept(s.recentRuns),
          live: Object.fromEntries(Object.entries(s.live).filter(([, run]) => !unfinished(run))),
        };
      });
      deps.rereadUsage();
      return count;
    },
    saveSeveral: async (outputIds) => {
      const folder = await openDialog({ directory: true, multiple: false });
      if (typeof folder !== "string") return null;
      let failed: unknown = null;
      for (const id of outputIds) {
        const output = runOfOutput(get(), id)?.outputs.find((o) => o.id === id);
        if (!output) continue;
        try {
          await api.studioExport(id, joinPath(folder, fileNameOf(output.path)), false);
        } catch (e) {
          failed ??= e;
        }
      }
      if (failed !== null) throw failed;
      return folder;
    },
  };
}

/** Whether `model` takes a picture as `role`. */
function takesRole(model: MediaModel, role: RefRole): boolean {
  return pictureRoles(model).some((r) => r.role === role && r.max > 0);
}

/** The model to offer `tab` for a picture as `role`, which the draft's model does not take: an
 *  installed local model that takes it (one that fits first; never one not for this computer),
 *  else the cloud model used most recently that takes it, else the first an endpoint lists. */
function modelTaking(
  s: Pick<StudioState, "local" | "endpoints" | "recentRuns">,
  tab: StudioTab,
  role: RefRole,
): { choice: ModelChoice; model: MediaModel } | null {
  const locals = localModelsFor(tab, s.local).filter((m) => m.fit.word !== "notHere" && takesRole(m.model, role));
  const local = locals.find((m) => FITTING.has(m.fit.word)) ?? locals[0];
  if (local) return { choice: { target: "thisComputer", model: local.model.id }, model: local.model };
  for (const choice of recentChoices(s.recentRuns, tab, RECENT_RUNS)) {
    const model = offeredModel(tab, choice.target, choice.model, s.local, s.endpoints);
    if (model && takesRole(model, role)) return { choice, model };
  }
  for (const endpoint of s.endpoints) {
    const model = cloudModelsFor(endpoint, tab).find((m) => takesRole(m, role));
    if (model) return { choice: { target: { endpoint: endpoint.providerId }, model: model.id }, model };
  }
  return null;
}

/** The run that made result `outputId`, at its newest state (the live runs first, then the
 *  gallery's and the newest runs); `null` when the store holds none. */
export function runOfOutput(s: Pick<StudioState, "live" | "runs" | "recentRuns">, outputId: string): Run | null {
  const made = (run: Run) => run.outputs.some((o) => o.id === outputId);
  return Object.values(s.live).find(made) ?? s.runs.find(made) ?? s.recentRuns.find(made) ?? null;
}

/** A file's name, the last part of `path` (either separator). */
export function fileNameOf(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}

/** `name` in `folder`, joined with the separator the folder's path uses (a Windows path that
 *  has only backslashes keeps them). */
export function joinPath(folder: string, name: string): string {
  const separator = folder.includes("\\") && !folder.includes("/") ? "\\" : "/";
  return `${folder.replace(/[\\/]+$/, "")}${separator}${name}`;
}
