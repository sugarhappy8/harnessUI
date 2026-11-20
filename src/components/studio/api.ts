// Studio's commands and events (0.1.0): one wrapper per command in src-tauri/src/studio/
// {commands,engine_cmd,local_cmd}.rs, in their order there. Argument names are the Rust
// parameters' in camelCase, as Tauri reads them. Only the read commands' price and list
// updates (`studioRefreshModels`, `studioModelPrice`) and Hugging Face (`studioLocalSearch`,
// `studioLocalRepo`, `studioLocalFit` for a searched file) send anything anywhere.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  EngineProgress,
  EngineStatus,
  Estimate,
  ImageInput,
  LocalFit,
  LocalModels,
  LocalRepo,
  LocalSearchPage,
  LocalSource,
  MediaKind,
  MediaModel,
  Output,
  RefItem,
  RefRole,
  RequestPreview,
  Run,
  RunsCursor,
  RunsFilter,
  RunsPage,
  StartRequest,
  StudioEndpoint,
  StudioPrefs,
  StudioPrefsPatch,
  Usage,
} from "./types";

// ---- the read commands --------------------------------------------------------------------

/** Every endpoint that makes pictures or video, with its saved models. Sends nothing. */
export const studioModels = () => invoke<{ endpoints: StudioEndpoint[] }>("studio_models");
/** *Get the list*, *Update model lists*: fetches the endpoint's lists. */
export const studioRefreshModels = (providerId: number) =>
  invoke<{ endpoints: StudioEndpoint[] }>("studio_refresh_models", { providerId });
/** Picking an OpenRouter picture model, *Get the price*: fetches its price. */
export const studioModelPrice = (providerId: number, model: string) =>
  invoke<MediaModel>("studio_model_price", { providerId, model });
/** What a run would cost and meet. Sends nothing. */
export const studioEstimate = (req: StartRequest) => invoke<Estimate>("studio_estimate", { req });
/** "See the exact request" (or command). Sends nothing. */
export const studioPreview = (req: StartRequest) => invoke<RequestPreview>("studio_preview", { req });
/** One page of the gallery's runs, newest first, after `before` (the previous page's `next`). */
export const studioRuns = (filter: RunsFilter, before: RunsCursor | null, limit: number) =>
  invoke<RunsPage>("studio_runs", { filter, before, limit });
/** The runs still working, oldest first (events are not replayed). */
export const studioLive = () => invoke<Run[]>("studio_live");
/** A result's bytes (or its thumbnail's), for a `blob:` URL. */
export const studioOutputBytes = (outputId: string, thumb: boolean) =>
  invoke<ArrayBuffer>("studio_output_bytes", { outputId, thumb });
/** An added picture's bytes. */
export const studioRefBytes = (sha256: string) => invoke<ArrayBuffer>("studio_ref_bytes", { sha256 });
/** Today's spending, the limit, and what the gallery holds. */
export const studioUsage = () => invoke<Usage>("studio_usage");
/** Settings → Studio. */
export const studioGetPrefs = () => invoke<StudioPrefs>("studio_get_prefs");

// ---- the changing commands ----------------------------------------------------------------

/** Create: records the run and queues it; `confirmed` says the user confirmed it. */
export const studioStart = (req: StartRequest, confirmed: boolean) => invoke<Run>("studio_start", { req, confirmed });
/** Cancel, Stop, Stop watching. */
export const studioCancel = (runId: string) => invoke<Run>("studio_cancel", { runId });
/** *Get it anyway*, *Check again*: never creates. */
export const studioFetchAnyway = (runId: string) => invoke<Run>("studio_fetch_anyway", { runId });
/** Saves a video's poster frame as its thumbnail. */
export const studioSaveThumb = (outputId: string, jpegBase64: string) =>
  invoke<Output>("studio_save_thumb", { outputId, jpegBase64 });
export const studioStar = (outputId: string, starred: boolean) => invoke<Output>("studio_star", { outputId, starred });
/** Deletes results; today's spending stays as it is. */
export const studioDelete = (outputIds: string[]) => invoke<void>("studio_delete", { outputIds });
/** *Delete stopped and failed runs*: how many went. */
export const studioDeleteUnfinished = () => invoke<number>("studio_delete_unfinished");
/** Copies a result to the file chosen in the save dialog; where it was written. */
export const studioExport = (outputId: string, path: string, withSettings: boolean) =>
  invoke<string>("studio_export", { outputId, path, withSettings });
export const studioShowInFolder = (outputId: string) => invoke<void>("studio_show_in_folder", { outputId });
export const studioOpenFolder = () => invoke<void>("studio_open_folder");
/** Keeps a picture added to the prompt in the gallery. */
export const studioAddRef = (image: ImageInput) => invoke<RefItem>("studio_add_ref", { image });
/** *Drop a picture* through Tauri's native drop (the only one that carries files on Windows):
 *  keeps file `index` of the files last dropped on the window, as `studio_add_ref` keeps a
 *  picture. The backend reads only the paths it recorded; the webview never sends one. */
export const studioAddDropped = (index: number, role: RefRole | null) =>
  invoke<RefItem>("studio_add_dropped", { index, role });
/** *Use in prompt*. */
export const studioRefFromOutput = (outputId: string) => invoke<RefItem>("studio_ref_from_output", { outputId });
/** *Send to chat*: the picture as the composer takes one. */
export const studioOutputForChat = (outputId: string) => invoke<ImageInput>("studio_output_for_chat", { outputId });
/** Changes only the fields `patch` sends. */
export const studioSetPrefs = (patch: StudioPrefsPatch) => invoke<StudioPrefs>("studio_set_prefs", { patch });
/** "Unload the chat model?" (Settings → Studio's Ask): the answer for a local run. */
export const studioAnswerUnload = (runId: string, unload: boolean) =>
  invoke<Run>("studio_answer_unload", { runId, unload });
/** *Quit and stop*, asked while a local job works. */
export const studioQuitNow = () => invoke<void>("studio_quit_now");

// ---- the Studio engine --------------------------------------------------------------------

export const studioEngineStatus = () => invoke<EngineStatus>("studio_engine_status");
/** The Windows engine pack's download, once its licence is accepted. */
export const studioEngineDownload = (acceptLicense: boolean) =>
  invoke<void>("studio_engine_download", { acceptLicense });
export const studioEngineCancel = () => invoke<void>("studio_engine_cancel");
export const studioEngineRemove = () => invoke<void>("studio_engine_remove");

// ---- Get local models ---------------------------------------------------------------------

/** What this computer can run. Sends nothing. */
export const studioLocalModels = () => invoke<LocalModels>("studio_local_models");
/** A Hugging Face search, on Search or Enter. */
export const studioLocalSearch = (text: string, kind: MediaKind) =>
  invoke<LocalSearchPage>("studio_local_search", { text, kind });
/** A searched repository's files at its latest commit, when one result is opened. */
export const studioLocalRepo = (repo: string) => invoke<LocalRepo>("studio_local_repo", { repo });
/** A fit at a size and, for a clip, a length in seconds (0 for a picture). */
export const studioLocalFit = (source: LocalSource, width: number, height: number, seconds: number) =>
  invoke<LocalFit>("studio_local_fit", { source, width, height, seconds });
/** Downloads a model and returns its download group ID. */
export const studioLocalDownload = (source: LocalSource) =>
  invoke<string>("studio_local_download", { source });
export const studioLocalDownloadFile = (repo: string, commit: string, file: string) =>
  invoke<string>("studio_local_download_file", { repo, commit, file });
export const studioLocalDelete = (genModelId: string) => invoke<void>("studio_local_delete", { genModelId });
/** The note about local models, accepted. */
export const studioAcceptLocalNotice = () => invoke<StudioPrefs>("studio_accept_local_notice");

// ---- events -------------------------------------------------------------------------------

/** A run's new state, at most 4 per second per run. */
export function onStudioProgress(handler: (run: Run) => void): Promise<UnlistenFn> {
  return listen<Run>("studio-progress", (e) => handler(e.payload));
}
/** The Windows engine pack's download, at most every 300 ms, then once finished. */
export function onEngineProgress(handler: (p: EngineProgress) => void): Promise<UnlistenFn> {
  return listen<EngineProgress>("studio-engine-progress", (e) => handler(e.payload));
}
/** Files dropped on the window (Tauri's native drop): how many, and where, in CSS pixels from
 *  the webview's top left corner. */
export async function onNativeDrop(handler: (count: number, at: { x: number; y: number }) => void): Promise<UnlistenFn> {
  const { getCurrentWebview } = await import("@tauri-apps/api/webview");
  return getCurrentWebview().onDragDropEvent((e) => {
    if (e.payload.type !== "drop") return;
    const scale = window.devicePixelRatio || 1;
    handler(e.payload.paths.length, { x: e.payload.position.x / scale, y: e.payload.position.y / scale });
  });
}
/** Closing the main window, or quitting, while a local job runs. */
export function onQuitRequested(handler: () => void): Promise<UnlistenFn> {
  return listen("studio-quit-requested", () => handler());
}
