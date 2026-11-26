// What the mocked backend holds: built from the repo's own Studio fixtures
// (src/components/studio/fixtures/models.ts, generated from the Rust DTOs) plus a few made-up
// records so the screens look like a session in use: OpenRouter with a realistic picture
// catalog, installed local models, finished runs with pictures, one run in progress, Hugging
// Face search results, chats, and a loaded chat model.
import { LOCAL, LOCAL_RUN, MODELS, RUN } from "../../src/components/studio/fixtures/models";
import type {
  Estimate,
  LocalModels,
  LocalRepo,
  LocalSearchHit,
  MediaModel,
  Output,
  Run,
  StudioEndpoint,
  StudioPrefs,
  Usage,
} from "../../src/components/studio/types";
import type {
  ChatDetail,
  ChatSummary,
  DownloadGroup,
  InstalledModel,
  ModelOption,
  RuntimeStatus,
} from "../../src/lib/types";
import type { Scene } from "./pictures";

const params = new URLSearchParams(location.search);
/** `?notice=0`: the note about local models was never accepted (first download shows it). */
const noticeAccepted = params.get("notice") !== "0";

const now = Date.now();
const minutes = (n: number) => n * 60_000;

// ---- cloud models ----------------------------------------------------------------------------

const [flare, banana, k3, veo, grok, seedance] = MODELS as [
  MediaModel,
  MediaModel,
  MediaModel,
  MediaModel,
  MediaModel,
  MediaModel,
];

function like(base: MediaModel, patch: Partial<MediaModel>): MediaModel {
  return { ...base, notes: [], ...patch };
}

/** OpenRouter's picture and video lists: the fixture's six models plus a few more picture
 *  models, as a real catalog holds more than three. */
export const OPENROUTER_MODELS: MediaModel[] = [
  like(flare, { id: "openai/gpt-image-2.6", displayName: "GPT Image 2.6", created: 1794700000 }),
  flare,
  banana,
  like(banana, { id: "google/gemini-nano-banana-2.1-pro", displayName: "Nano Banana 2.1 Pro", notes: banana.notes }),
  like(banana, {
    id: "black-forest-labs/flux-3-image",
    displayName: "FLUX 3 Image",
    family: "black-forest-labs/flux-3",
  }),
  like(banana, {
    id: "black-forest-labs/flux-3-image-pro",
    displayName: "FLUX 3 Image Pro",
    family: "black-forest-labs/flux-3",
  }),
  like(banana, { id: "bytedance-seed/seedream-5", displayName: "Seedream 5", family: "bytedance-seed/seedream" }),
  k3,
  veo,
  grok,
  seedance,
];

export const ENDPOINTS: StudioEndpoint[] = [
  {
    providerId: 1,
    name: "OpenRouter",
    company: "openrouter",
    host: "openrouter.ai",
    cloud: true,
    hasKey: true,
    fetchedAt: now - 2 * 24 * 3600_000,
    models: OPENROUTER_MODELS,
  },
];

/** A model as `studio_model_price` answers once picked: its price known. */
export function priced(model: MediaModel): MediaModel {
  return {
    ...model,
    price: { unit: "image", low: 0.039, high: 0.039 },
    priceKnown: true,
    priceFetchedAt: Date.now(),
  };
}

// ---- local models -----------------------------------------------------------------------------

export const LOCAL_MODELS: LocalModels = {
  ...LOCAL,
  noticeAccepted,
  // The video model was run here once (the gallery's waves clip), so its time is measured.
  installed: LOCAL.installed.map((m) =>
    m.kind === "video" ? { ...m, fit: { ...m.fit, eta: { lowMs: 330_000, highMs: 400_000, measured: true, leftMs: null } } } : m,
  ),
};

// ---- runs -------------------------------------------------------------------------------------

/** Which sample picture each run's results show. */
export const SCENE_OF_RUN: Record<string, Scene> = {};

function outputs(runId: string, n: number, opts: Partial<Output> = {}): Output[] {
  return Array.from({ length: n }, (_, idx) => ({
    id: `${runId}-o${idx + 1}`,
    runId,
    idx,
    mime: "image/png",
    sizeBytes: 1_700_000 + idx * 23_117,
    width: 1024,
    height: 1024,
    durationMs: null,
    seed: 81723 + idx,
    starred: false,
    hasThumb: true,
    path: `/home/me/harnessUI/studio/2026-11/${runId}-${idx}.png`,
    ...opts,
  }));
}

function run(id: string, scene: Scene, patch: Partial<Run>): Run {
  SCENE_OF_RUN[id] = scene;
  return { ...RUN, id, outputs: [], ...patch } as Run;
}

const coastOutputs = outputs("coast", 4);
coastOutputs[1] = { ...coastOutputs[1]!, starred: true };

/** The gallery: finished runs, oldest first here (the backend pages them newest first). */
export const RUNS: Run[] = [
  run("coast", "coast", {
    ...RUN,
    id: "coast",
    providerName: "OpenRouter",
    createdAt: now - minutes(190),
    startedAt: now - minutes(190) + 200,
    finishedAt: now - minutes(190) + 13_800,
    outputs: coastOutputs,
  }),
  run("fox", "fox", {
    ...LOCAL_RUN,
    id: "fox",
    createdAt: now - minutes(160),
    startedAt: now - minutes(160) + 400,
    finishedAt: now - minutes(160) + 229_300,
    outputs: outputs("fox", 2),
  }),
  run("teapot", "teapot", {
    ...RUN,
    id: "teapot",
    seq: 5,
    providerName: "OpenRouter",
    upstream: "Google",
    model: banana.id,
    modelLabel: banana.displayName,
    prompt: "A ceramic teapot shaped like a snail, studio product photo, soft window light",
    params: { aspect_ratio: "1:1", resolution: "1K" },
    costUsd: 0.039,
    createdAt: now - minutes(130),
    startedAt: now - minutes(130) + 300,
    finishedAt: now - minutes(130) + 9_400,
    outputs: outputs("teapot", 1),
  }),
  run("failed", "coast", {
    ...RUN,
    id: "failed",
    seq: 6,
    providerName: "OpenRouter",
    model: flare.id,
    modelLabel: flare.displayName,
    prompt: "Isometric cutaway of a tiny bookshop, warm evening light",
    params: { aspect_ratio: "4:3", n: 2, quality: "medium" },
    status: "failed",
    costUsd: null,
    remoteId: null,
    error: {
      code: "studio_not_answered",
      message: "OpenRouter did not answer in time, or the connection was cut.",
      nextStep: "OpenRouter does not charge for a picture you did not receive. Try again.",
      details: "Request id: gen-1794914300-Xq81LmZa0pDe",
      charged: false,
      requestId: "gen-1794914300-Xq81LmZa0pDe",
      company: "openrouter",
      stage: null,
    },
    createdAt: now - minutes(95),
    startedAt: now - minutes(95) + 200,
    finishedAt: now - minutes(95) + 120_000,
    outputs: [],
  }),
  run("city", "city", {
    ...LOCAL_RUN,
    id: "city",
    seq: 7,
    prompt: "Rainy neon street in Tokyo at night, cinematic, reflections on wet asphalt",
    params: { aspect_ratio: "1:1", n: 2, seed: 4412 },
    createdAt: now - minutes(40),
    startedAt: now - minutes(40) + 400,
    finishedAt: now - minutes(40) + 232_000,
    outputs: outputs("city", 2),
    local: { ...LOCAL_RUN.local!, elapsedMs: 231_000 },
  }),
  run("waves", "waves", {
    ...LOCAL_RUN,
    id: "waves",
    seq: 8,
    kind: "video",
    model: "wan22-ti2v-5b-fast-q6",
    modelLabel: "Wan 2.2 5B, fast",
    prompt: "Waves rolling onto a black sand beach, slow motion",
    params: { aspect_ratio: "16:9", resolution: "480p", duration: 2 },
    createdAt: now - minutes(70),
    startedAt: now - minutes(70) + 400,
    finishedAt: now - minutes(70) + 370_000,
    outputs: outputs("waves", 1, { mime: "video/webm", width: 832, height: 480, durationMs: 2000 }),
    local: { ...LOCAL_RUN.local!, elapsedMs: 369_000 },
  }),
  run("balloons", "balloons", {
    ...RUN,
    id: "balloons",
    seq: 9,
    kind: "video",
    providerName: "OpenRouter",
    upstream: "Google",
    model: veo.id,
    modelLabel: veo.displayName,
    prompt: "Hot air balloon festival at sunrise, slow drone shot",
    params: { aspect_ratio: "16:9", duration: 8, resolution: "720p", generate_audio: false },
    costUsd: 1.6,
    createdAt: now - minutes(55),
    startedAt: now - minutes(55) + 200,
    finishedAt: now - minutes(55) + 96_000,
    outputs: outputs("balloons", 1, { mime: "video/mp4", width: 1280, height: 720, durationMs: 8000 }),
  }),
];

/** The run in progress: a local picture, the first of two, step 3 of 4. */
export function liveRun(): Run {
  SCENE_OF_RUN.lake = "lake";
  const started = Date.now() - 21_000;
  return {
    ...LOCAL_RUN,
    id: "lake",
    seq: 50,
    prompt: "Misty mountain lake at sunrise, oil painting",
    params: { aspect_ratio: "1:1", n: 2 },
    status: "running",
    progress: 0.38,
    costUsd: null,
    createdAt: started - 1500,
    startedAt: started,
    finishedAt: null,
    outputs: [],
    eta: { lowMs: 90_000, highMs: 120_000, measured: true, leftMs: 34_000 },
    local: {
      ...LOCAL_RUN.local!,
      stage: "drawing",
      step: 3,
      steps: 4,
      picture: 1,
      pictures: 2,
      elapsedMs: null,
    },
  };
}

// ---- preferences, usage, estimates -------------------------------------------------------------

export const PREFS: StudioPrefs = {
  lastModel: {
    image: { target: "thisComputer", model: "flux2-klein-4b-q4" },
    video: { target: "thisComputer", model: "wan22-ti2v-5b-fast-q6" },
  },
  advancedOpen: false,
  confirmEveryVideo: true,
  confirmAtUsd: 1,
  dailyLimitUsd: 10,
  setups: [
    {
      name: "Watercolour 4-up",
      kind: "image",
      target: { endpoint: 1 },
      model: flare.id,
      params: { aspect_ratio: "1:1", n: 4, quality: "low" },
      extra: {},
      routing: null,
    },
  ],
  galleryDir: null,
  gpuPolicy: "unload",
  reloadChatAfter: false,
  suggestionDismissed: true,
  localNoticeAcceptedAt: noticeAccepted ? now - 5 * 24 * 3600_000 : null,
};

export const USAGE: Usage = {
  spentTodayUsd: 0.08,
  pendingUsd: 0,
  limitUsd: 10,
  folder: "/home/me/harnessUI/studio",
  bytes: 61_400_000,
  pictures: 11,
  videos: 2,
};

export function estimateFor(req: { target: unknown; kind: string; params: Record<string, unknown> }): Estimate {
  const limit = { limitUsd: 10, spentTodayUsd: 0.08, pendingUsd: 0, wouldPass: true, refusal: null, reserveUsd: 5 };
  if (req.target === "thisComputer") {
    return {
      usd: 0,
      reasons: [],
      confirmAtUsd: 1,
      limit,
      invalid: [],
      eta:
        req.kind === "video"
          ? { lowMs: 330_000, highMs: 400_000, measured: true, leftMs: null }
          : { lowMs: 90_000, highMs: 120_000, measured: true, leftMs: null },
      local: { fit: "fits", unloadChat: false, verySlow: false, over15Min: false, reason: null },
    };
  }
  const n = typeof req.params.n === "number" ? req.params.n : 1;
  return {
    usd: req.kind === "video" ? 1.6 : 0.039 * n,
    reasons: req.kind === "video" ? ["video"] : [],
    confirmAtUsd: 1,
    limit,
    invalid: [],
    eta: null,
    local: null,
  };
}

// ---- Hugging Face ---------------------------------------------------------------------------------

export const SEARCH_HITS: LocalSearchHit[] = [
  { repo: "leejet/Z-Image-Turbo-GGUF", downloads: 48_213, likes: 312, gated: "none", licenseLabel: "apache-2.0" },
  { repo: "jayn7/Z-Image-Turbo-GGUF", downloads: 21_904, likes: 96, gated: "none", licenseLabel: "apache-2.0" },
  { repo: "Tongyi-MAI/Z-Image-Turbo", downloads: 402_117, likes: 2_841, gated: "none", licenseLabel: "apache-2.0" },
  { repo: "unsloth/Z-Image-Turbo-GGUF", downloads: 9_377, likes: 41, gated: "none", licenseLabel: "apache-2.0" },
  { repo: "Comfy-Org/z_image_turbo", downloads: 156_220, likes: 488, gated: "none", licenseLabel: "apache-2.0" },
  { repo: "someone/Z-Image-Turbo-anime-LoRA", downloads: 3_208, likes: 57, gated: "none", licenseLabel: "other" },
];

const zimage = LOCAL.recommended.find((e) => e.id === "z-image-turbo-q4")!;
const GIB = 1024 ** 3;

function fileChoice(file: string, quant: string, diffusionGb: number) {
  const parts = [
    { ...zimage.parts[0]!, repo: "leejet/Z-Image-Turbo-GGUF", path: file, sizeBytes: Math.round(diffusionGb * GIB) },
    { ...zimage.parts[1]! },
    { ...zimage.parts[2]! },
  ];
  const total = parts.reduce((n, p) => n + p.sizeBytes, 0);
  return {
    file,
    quant,
    parts,
    licences: [zimage.licence],
    totalBytes: total,
    toFetchBytes: total,
    fit: { ...zimage.fit, verdict: diffusionGb > 6 ? ("yellow" as const) : ("green" as const), word: "fits" as const },
    needsEngine: false,
    lowDisk: false,
  };
}

export function repoFor(repo: string): LocalRepo {
  const commit = "3f1c9a7d52be0e41a0c8f6d2b7e9a1c4d5f60718";
  if (repo.endsWith("-GGUF")) {
    return {
      repo,
      commit,
      status: "supported",
      reason: null,
      family: "Z-Image Turbo",
      files: [
        fileChoice("z_image_turbo-Q4_K.gguf", "Q4_K", 3.6),
        fileChoice("z_image_turbo-Q5_K_M.gguf", "Q5_K_M", 4.3),
        fileChoice("z_image_turbo-Q8_0.gguf", "Q8_0", 6.7),
      ],
      standaloneFiles: [],
      licence: zimage.licence,
      licences: [zimage.licence],
      labelMismatch: null,
      gate: { mode: "none", collects: [] },
    };
  }
  return {
    repo,
    commit,
    status: "notSupportedYet",
    reason: "Studio cannot run this repository's files yet.",
    family: null,
    files: [],
    standaloneFiles: [
      { path: "z_image_turbo_bf16.safetensors", size: Math.round(12.3 * GIB), sha256: "a".repeat(64) },
      { path: "split_files/vae/ae.safetensors", size: 335_304_388, sha256: "b".repeat(64) },
      { path: "split_files/text_encoders/qwen_3_4b.safetensors", size: Math.round(8.0 * GIB), sha256: "c".repeat(64) },
    ],
    licence: zimage.licence,
    licences: [zimage.licence],
    labelMismatch: null,
    gate: { mode: "none", collects: [] },
  };
}

/** Z-Image Turbo downloading, once Download was pressed in the confirmation. */
export function zimageDownload(): DownloadGroup {
  return {
    id: "group-1",
    repo: "leejet/Z-Image-Turbo-GGUF",
    label: "Z-Image Turbo",
    fileCount: 3,
    bytesDone: Math.round(2.2 * GIB),
    bytesTotal: zimage.sizeBytes,
    status: "downloading",
    error: null,
    bytesPerSecond: 38_000_000,
    unverified: false,
    generatorKind: "image",
    genModelId: "z-image-turbo-q4",
  };
}

// ---- the chat side ---------------------------------------------------------------------------------

export const CHATS: ChatSummary[] = [
  { id: 7, title: "Refactor the download queue", updatedAt: now - minutes(25), snippet: "Split the queue into a scheduler and workers…" },
  { id: 6, title: "Lisbon in March: 4-day plan", updatedAt: now - minutes(180), snippet: "Day 1: Alfama, the castle and a fado dinner" },
  { id: 5, title: "Vulkan or ROCm on a 7600?", updatedAt: now - 26 * 3600_000, snippet: "For llama.cpp on an RX 7600, Vulkan is…" },
  { id: 4, title: "Regex for nginx access logs", updatedAt: now - 30 * 3600_000, snippet: null },
  { id: 3, title: "Summarise the release notes", updatedAt: now - 4 * 24 * 3600_000, snippet: "0.1.2 expands Studio's Hugging Face downloads" },
  { id: 2, title: "Sourdough schedule", updatedAt: now - 9 * 24 * 3600_000, snippet: null },
];

const MODEL_PATH = "/home/me/models/Qwen3-8B-Q4_K_M.gguf";

export const RUNTIME: RuntimeStatus = {
  state: "ready",
  backend: "vulkan",
  device: "Vulkan0",
  modelPath: MODEL_PATH,
  modelName: "Qwen3 8B",
  progress: null,
  stage: null,
  nCtx: 16384,
  supportsTools: true,
  preserveReasoning: false,
  thinking: true,
  speedTps: 41.2,
  slow: false,
  vision: false,
  offloadedLayers: 37,
  totalLayers: 37,
  error: null,
  loadStage: null,
  adjustments: [],
  ctxSource: "automatic",
  settingsChanged: false,
  reloadWaiting: false,
  waitingForStudio: false,
  unloadedPath: null,
  placement: null,
};

export const MODEL_OPTIONS: ModelOption[] = [
  { ref: `local:${MODEL_PATH}`, label: "Qwen3-8B-Q4_K_M.gguf", source: "local", contextLength: 40960, recent: true },
  { ref: "local:/home/me/models/gemma-4-12b-it-Q4_K_M.gguf", label: "gemma-4-12b-it-Q4_K_M.gguf", source: "local", contextLength: 131072, recent: true },
  { ref: "provider:2:anthropic/claude-sonnet-4.5", label: "anthropic/claude-sonnet-4.5", source: "provider", contextLength: 200000 },
  { ref: "provider:2:openai/gpt-5.1", label: "openai/gpt-5.1", source: "provider", contextLength: 400000 },
];

export function chatDetail(id: number): ChatDetail {
  const summary = CHATS.find((c) => c.id === id) ?? CHATS[0]!;
  const t = summary.updatedAt;
  const base = {
    reasoning: null,
    status: "complete" as const,
    modelRef: `local:${MODEL_PATH}`,
    promptTokens: null,
    completionTokens: null,
    tokensPerSecond: null,
    error: null,
    imageIds: [],
    documents: [],
    toolCalls: [],
    toolResult: null,
  };
  return {
    chat: {
      id: summary.id,
      title: summary.title,
      modelRef: `local:${MODEL_PATH}`,
      systemPrompt: null,
      leafMessageId: 102,
      createdAt: t - minutes(10),
      updatedAt: t,
      thinking: false,
      workingFolder: null,
    },
    messages: [
      {
        ...base,
        id: 101,
        parentId: null,
        role: "user",
        text: "The download queue in our app is one big loop that both schedules and downloads. How would you split it?",
        createdAt: t - minutes(9),
        siblingIds: [101],
      },
      {
        ...base,
        id: 102,
        parentId: 101,
        role: "assistant",
        text:
          "Split it into a **scheduler** and a small pool of **workers**:\n\n" +
          "1. The scheduler owns the queue and decides what runs next (priority, pause, resume).\n" +
          "2. Each worker downloads one file and reports progress on a channel.\n" +
          "3. The UI listens to one event stream, so it never polls.\n\n" +
          "```rust\nlet (tx, rx) = mpsc::channel(32);\nfor _ in 0..workers { spawn(worker(rx.clone(), tx.clone())); }\n```\n\n" +
          "That keeps cancellation simple: the scheduler drops a job, and its worker stops at the next chunk.",
        completionTokens: 182,
        tokensPerSecond: 40.6,
        createdAt: t - minutes(8),
        siblingIds: [102],
      },
    ],
    summary: null,
  };
}

export const INSTALLED_CHAT_MODELS: InstalledModel[] = [
  {
    id: 1,
    path: MODEL_PATH,
    repo: "Qwen/Qwen3-8B-GGUF",
    fileName: "Qwen3-8B-Q4_K_M.gguf",
    sizeBytes: 5_027_783_488,
    source: "download",
    projectorPath: null,
    summary: {
      architecture: "qwen3",
      quantization: "Q4_K_M",
      sizeLabel: "8B",
      contextLength: 40960,
      templateTools: true,
      templateThinking: true,
      isMoe: false,
    },
    architectureSupported: true,
    loadedAs: MODEL_PATH,
  },
  {
    id: 2,
    path: "/home/me/models/gemma-4-12b-it-Q4_K_M.gguf",
    repo: "google/gemma-4-12b-it-GGUF",
    fileName: "gemma-4-12b-it-Q4_K_M.gguf",
    sizeBytes: 7_300_000_000,
    source: "download",
    projectorPath: null,
    summary: {
      architecture: "gemma4",
      quantization: "Q4_K_M",
      sizeLabel: "12B",
      contextLength: 131072,
      templateTools: true,
      templateThinking: false,
      isMoe: false,
    },
    architectureSupported: true,
  },
];
