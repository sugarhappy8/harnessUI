// The mocked backend: every Tauri command the app invokes goes through `handle`. Commands it
// does not know answer `null` and are recorded in `window.__preview.unhandled` (and logged once),
// so a screen that needs a new command shows up there; add a case below for it.
import { emit } from "@tauri-apps/api/event";
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import type { Lora, Run, RunsFilter, SearchFilter, StudioPrefs } from "../../../src/components/studio/types";
import {
  CHATS,
  chatDetail,
  ENDPOINTS,
  estimateFor,
  INSTALLED_CHAT_MODELS,
  liveRun,
  LOCAL_MODELS,
  MODEL_OPTIONS,
  PREFS,
  priced,
  filesOf,
  planFor,
  repoFor,
  RUNS,
  RUNTIME,
  SCENE_OF_RUN,
  searchFor,
  USAGE,
  zimageDownload,
} from "./data";
import { pictureBytes } from "./pictures";

type Args = Record<string, unknown> | undefined;

/** `?empty=1`: Studio as a new user finds it: nothing installed, no endpoint, no runs. */
const empty = new URLSearchParams(location.search).get("empty") === "1";
/** `?fits=0` (with `empty=1`): nothing in the catalog fits this computer either, so the empty
 *  state is "No models on this computer yet" with *Get models* (Task U9). */
const nothingFits = new URLSearchParams(location.search).get("fits") === "0";
/** `?pictures=1` (with `empty=1`): the picture models stay installed, and no video model is
 *  installed or fits, so the Video tab says "No video models on this computer yet" (U9 fix
 *  round 1). */
const picturesOnly = new URLSearchParams(location.search).get("pictures") === "1";
const uninstalled = LOCAL_MODELS.recommended.map((e) => ({ ...e, installed: false }));

const state = {
  prefs: (empty ? { ...PREFS, lastModel: {}, setups: [] } : { ...PREFS }) as StudioPrefs,
  local:
    empty && picturesOnly
      ? {
          ...LOCAL_MODELS,
          installed: LOCAL_MODELS.installed.filter((m) => m.kind === "image"),
          recommended: LOCAL_MODELS.recommended.filter((e) => e.kind === "image"),
          notHere: [...LOCAL_MODELS.notHere, ...uninstalled.filter((e) => e.kind === "video")],
        }
      : empty
        ? {
            ...LOCAL_MODELS,
            installed: [],
            installedBytes: 0,
            moreFit: nothingFits ? 0 : 2,
            recommended: nothingFits ? [] : uninstalled,
            notHere: nothingFits ? [...uninstalled, ...LOCAL_MODELS.notHere] : LOCAL_MODELS.notHere,
          }
        : LOCAL_MODELS,
  endpoints: empty ? [] : ENDPOINTS,
  runs: empty ? [] : RUNS.map((r) => ({ ...r })),
  live: empty ? [] : [liveRun()],
  downloads: [] as ReturnType<typeof zimageDownload>[],
};

const unhandled = new Set<string>();

function matches(run: Run, filter: RunsFilter): boolean {
  const text = filter.text?.trim().toLowerCase() ?? "";
  return (
    (!filter.kind || run.kind === filter.kind) &&
    (!filter.model || run.model === filter.model) &&
    (filter.since == null || run.createdAt >= filter.since) &&
    (filter.until == null || run.createdAt < filter.until) &&
    (!filter.starred || run.outputs.some((o) => o.starred)) &&
    (text === "" || run.prompt.toLowerCase().includes(text))
  );
}

function outputById(id: string) {
  for (const run of [...state.runs, ...state.live]) {
    const output = run.outputs.find((o) => o.id === id);
    if (output) return { run, output };
  }
  return null;
}

async function handle(cmd: string, args: Args): Promise<unknown> {
  // IPC payloads are untyped here; each case reads the arguments api.ts sends.
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const a = (args ?? {}) as Record<string, any>;
  switch (cmd) {
    // ---- Studio ----------------------------------------------------------------------------
    case "studio_models":
    case "studio_refresh_models":
      return { endpoints: state.endpoints };
    case "studio_model_price": {
      const model = ENDPOINTS[0]!.models.find((m) => m.id === a.model)!;
      return priced(model);
    }
    case "studio_estimate":
      return estimateFor(a.req);
    case "studio_preview":
      return a.req?.target === "thisComputer"
        ? { method: "run", url: "sd-cli", host: "", body: { args: ["--diffusion-model", "flux-2-klein-4b-Q4_0.gguf", "--steps", "4"] } }
        : { method: "POST", url: "https://openrouter.ai/api/v1/images", host: "openrouter.ai", body: { model: a.req?.model, prompt: a.req?.prompt } };
    case "studio_runs": {
      const filter = (a.filter ?? {}) as RunsFilter;
      const runs = state.runs.filter((r) => matches(r, filter)).sort((x, y) => y.createdAt - x.createdAt);
      return { runs: runs.slice(0, a.limit ?? 60), next: null };
    }
    case "studio_live":
      return state.live;
    case "studio_output_bytes": {
      const found = outputById(a.outputId);
      const scene = found ? (SCENE_OF_RUN[found.run.id] ?? "coast") : "coast";
      const w = found?.output.width ?? 1024;
      const h = found?.output.height ?? 1024;
      return a.thumb
        ? pictureBytes(a.outputId, scene, Math.round((384 * w) / Math.max(w, h)), Math.round((384 * h) / Math.max(w, h)), "image/jpeg")
        : pictureBytes(a.outputId, scene, w, h, "image/png");
    }
    case "studio_ref_bytes":
      return pictureBytes(String(a.sha256), "teapot", 256, 256, "image/png");
    case "studio_usage":
      return empty ? { ...USAGE, spentTodayUsd: 0, bytes: 0, pictures: 0, videos: 0 } : USAGE;
    case "studio_get_prefs":
      return state.prefs;
    case "studio_set_prefs":
      state.prefs = { ...state.prefs, ...(a.patch ?? {}) };
      return state.prefs;
    case "studio_star": {
      const found = outputById(a.outputId);
      if (found) found.output.starred = Boolean(a.starred);
      return found?.output ?? null;
    }
    case "studio_engine_status":
      return state.local.engine;
    case "studio_local_models":
      return { ...state.local, noticeAccepted: state.prefs.localNoticeAcceptedAt !== null };
    case "studio_local_search":
      await new Promise((r) => setTimeout(r, 150));
      return searchFor((a.filter ?? "pictures") as SearchFilter);
    case "studio_local_repo":
      await new Promise((r) => setTimeout(r, 150));
      return repoFor(String(a.repo));
    case "studio_setup_plan":
      return planFor(state.local.installed, a.source, a.choice ?? null);
    case "studio_import_inspect": {
      const path = String(a.path);
      const name = path.split("/").pop() ?? path;
      const isLora = /lora/i.test(name);
      return {
        path,
        name,
        sizeBytes: isLora ? 170_000_000 : 6_938_078_334,
        label: isLora
          ? { lora: { base: "sdxl" } }
          : { works: { version: "sdxl", recipe: "sdxl-checkpoint", kind: "picture", part: null, parameters: 3.5e9, folder: false } },
        plan: isLora ? null : planFor(state.local.installed, { disk: { path } }, null),
        lastMode: state.prefs.lastImportMode ?? "copy",
        insideModels: false,
        byHandRecipes: [],
      };
    }
    case "studio_import_model":
      state.prefs = { ...state.prefs, lastImportMode: a.mode };
      return { genModelId: "local:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd", downloadGroup: null };
    case "studio_model_files":
      return filesOf(state.local.installed, String(a.genModelId));
    case "studio_set_model_files":
      return { files: filesOf(state.local.installed, String(a.genModelId), a.choice?.recipe ?? null), downloadGroup: null };
    case "studio_loras":
      return state.local.loras;
    case "studio_lora_download": {
      const lora: Lora = {
        id: `hf:${a.repo}:${a.file}`,
        name: String(a.file).replace(/\.safetensors$/, ""),
        base: "sdxl",
        format: "kohya",
        sizeBytes: 170_000_000,
        source: "search",
        repo: String(a.repo),
        missing: false,
      };
      state.local = { ...state.local, loras: [lora, ...state.local.loras] };
      return "group-lora-1";
    }
    case "studio_lora_import": {
      const path = String(a.path);
      const lora: Lora = {
        id: "local:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
        name: (path.split("/").pop() ?? path).replace(/\.safetensors$/, ""),
        base: "sdxl",
        format: "kohya",
        sizeBytes: 170_000_000,
        source: "import",
        repo: null,
        missing: false,
      };
      state.local = { ...state.local, loras: [lora, ...state.local.loras] };
      return lora;
    }
    case "studio_lora_delete":
      state.local = { ...state.local, loras: state.local.loras.filter((l) => l.id !== a.id) };
      return null;
    case "studio_local_delete":
      state.local = { ...state.local, installed: state.local.installed.filter((m) => m.id !== a.genModelId) };
      return null;
    case "plugin:dialog|open":
      return "/home/me/Downloads/my model.safetensors";
    case "studio_local_fit": {
      // A clip's time grows with its size and length (a picture's with its size).
      const pixels = (Number(a.width) || 832) * (Number(a.height) || 480);
      const seconds = Number(a.seconds) || 2;
      const ms = 60_000 + seconds * 140_000 * (pixels / (832 * 480));
      return { word: ms > 900_000 ? "slow" : "fits", verdict: ms > 900_000 ? "yellow" : "green", reason: null, suggestion: null, eta: { lowMs: ms * 0.9, highMs: ms * 1.1, measured: true, leftMs: null }, verySlow: false };
    }
    case "studio_local_download":
    case "studio_local_download_file":
      state.downloads = [zimageDownload()];
      return "group-1";
    case "studio_accept_local_notice":
      state.prefs = { ...state.prefs, localNoticeAcceptedAt: Date.now() };
      return state.prefs;
    case "studio_start":
    case "studio_cancel":
    case "studio_fetch_anyway":
      return state.live[0];
    case "studio_delete":
    case "studio_open_folder":
    case "studio_show_in_folder":
      return null;

    // ---- the app shell and chat ------------------------------------------------------------
    case "app_info":
      return { name: "harnessUI", version: "0.1.2", identifier: "app.harnessui", os: "linux", arch: "x86_64", commit: "967f38f" };
    case "first_run_info":
      return { done: true, hardware: { target: "gpu", gpuName: "Radeon RX 7600", gpuFreeBytes: 7e9, ramTotalBytes: 34359738368, ramAvailableBytes: 2e10, usableBytes: 7e9, backend: "vulkan", intelMac: false }, pick: null, suggestions: [], catalogSequence: 1, refreshDisabledReason: null };
    case "list_chats":
      return a.query ? CHATS.filter((c) => c.title.toLowerCase().includes(String(a.query).toLowerCase())) : CHATS;
    case "get_chat":
      return chatDetail(Number(a.chatId ?? a.id));
    case "runtime_status":
      return RUNTIME;
    case "list_model_options":
      return MODEL_OPTIONS;
    case "pending_approvals":
      return [];
    case "running_turn":
      return null;
    case "list_downloads":
      return state.downloads;
    // Pause, Resume and Cancel on a download (Task U7): its new state, as the download manager
    // would send it.
    case "download_pause":
    case "download_resume":
    case "download_cancel": {
      const status = cmd === "download_pause" ? "paused" : cmd === "download_resume" ? "downloading" : "cancelled";
      state.downloads = state.downloads.map((g) => (g.id === a.groupId ? { ...g, status } : g));
      const changed = state.downloads.find((g) => g.id === a.groupId);
      if (changed) void emit("download-progress", changed);
      return null;
    }
    case "list_installed_models":
      return INSTALLED_CHAT_MODELS;
    case "disk_usage":
      return { modelsDir: "/home/me/models", totalBytes: 12_327_783_488 };
    case "get_setting":
      return null;
    case "list_providers":
      return [
        { id: 1, name: "OpenRouter", kind: "openrouter", baseUrl: "https://openrouter.ai/api/v1", hasApiKey: true, enabled: true, cloud: true },
      ];
    case "list_backends":
      return [];
    case "list_chat_tools":
      return [];
    case "list_approved_folders":
      return [];
    case "loaded_skills":
      return [];
    case "list_audit":
      return [];
    case "get_chat_approval_mode":
    case "get_default_approval_mode":
      return "ask";
    case "update_status":
      return { checking: false, checked: true, downloading: null, available: null, ready: null, error: null, configured: true, autoCheck: true, currentVersion: "0.1.2" };
    case "stop_hotkey_status":
      return { accelerator: "Ctrl+Shift+Escape", registered: true, error: null };
    case "usage_now":
      return {
        graphics: { kind: "value", value: { name: "Radeon RX 7600", busyPercent: { kind: "value", value: 4 }, memoryUsedBytes: { kind: "value", value: 5.6e9 }, memoryTotalBytes: 8589934592 } },
        processorPercent: { kind: "value", value: 7 },
        threads: 16,
        memory: { kind: "value", value: { usedBytes: 14.2e9, totalBytes: 34359738368 } },
        unified: false,
      };
    case "artifact_isolation":
      return { state: "proven" };
    case "list_sources":
      return [];
    // Read by the chat screen; nothing on the screens taken depends on them.
    case "share_status":
    case "git_info":
    case "reverse_dropped":
    case "model_load_info":
      return null;
    case "plugin:event|listen":
    case "plugin:event|unlisten":
    case "plugin:event|emit":
      return null;
  }
  if (!unhandled.has(cmd)) {
    unhandled.add(cmd);
    console.info(`[preview] unhandled command ${cmd}`, args);
  }
  return null;
}

export function installMocks() {
  mockWindows("main");
  mockIPC((cmd, args) => handle(cmd, args as Args), { shouldMockEvents: true });
  (window as unknown as { __preview: unknown }).__preview = {
    unhandled,
    state,
    /** Sends an app event, as the backend would (`studio-progress`, `download-progress`…). */
    emit: (event: string, payload: unknown) => emit(event, payload),
  };
}
