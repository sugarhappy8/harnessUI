// What the mocked backend holds: built from the repo's own Studio fixtures
// (src/components/studio/fixtures/models.ts, generated from the Rust DTOs) plus a few made-up
// records so the screens look like a session in use: OpenRouter with a realistic picture
// catalog, installed local models, finished runs with pictures, one run in progress, Hugging
// Face search results, chats, and a loaded chat model.
import { LOCAL, LOCAL_RUN, MODELS, RUN } from "../../../src/components/studio/fixtures/models";
import type {
  Estimate,
  FileLabel,
  Gate,
  InstalledLocal,
  Licence,
  LocalFileChoice,
  LocalFit,
  LocalModels,
  LocalRepo,
  LocalSearchHit,
  LocalSearchPage,
  LocalSource,
  Lora,
  MediaModel,
  ModelFiles,
  Output,
  ParamSpec,
  Part,
  PartRole,
  PartSource,
  PlannedSlot,
  RecipeChoice,
  RecipeKind,
  RepoFileView,
  Run,
  SdVersion,
  SearchFilter,
  SetupChoice,
  SetupPlan,
  StudioEndpoint,
  StudioPrefs,
  Usage,
  Verdict,
} from "../../../src/components/studio/types";
import type {
  ChatDetail,
  ChatSummary,
  DownloadGroup,
  InstalledModel,
  ModelOption,
  RuntimeStatus,
} from "../../../src/lib/types";
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

const sdxl = LOCAL.installed.find((m) => m.version === "sdxl")!;

/** A picture field of a model on this computer, as the backend builds one (`picture_field` in
 *  src-tauri/src/studio/local.rs): one picture of `role`, required. */
function pictureField(key: string, label: string, role: "source" | "mask", help: string | null): ParamSpec {
  return {
    key,
    canonical: null,
    label,
    help,
    layer: "simple",
    kind: { type: "images", min: 1, max: 1, role },
    companyDefault: null,
    initial: null,
    affectsPrice: false,
    requires: null,
  };
}

/** An SDXL inpainting model, installed from a search: a picture-edit model, which asks for the
 *  picture to change and its mask (its recipe's `input_counts`). */
const SDXL_INPAINT: InstalledLocal = {
  ...sdxl,
  id: "hf:diffusers/stable-diffusion-xl-1.0-inpainting-0.1:sd_xl_inpainting_0.1.safetensors",
  name: "sd_xl_inpainting_0.1",
  sizeBytes: 5_140_000_000,
  recipe: { id: "sdxl-inpaint", name: "SDXL inpainting", kind: "pictureEdit" },
  version: "sdxl-inpaint",
  model: {
    ...sdxl.model,
    id: "sd_xl_inpainting_0.1",
    displayName: "sd_xl_inpainting_0.1",
    params: [
      ...sdxl.model.params.filter((p) => p.kind.type !== "images"),
      pictureField("source_picture", "Picture to change", "source", null),
      pictureField("mask_picture", "Mask", "mask", "Any picture: its white areas are the ones changed."),
    ],
  },
};

/** A second LoRA, downloaded from Hugging Face (the fixture's is added from this computer). */
const ANIME_LORA: Lora = {
  id: "hf:someone/Z-Image-Turbo-anime-LoRA:anime-style-v2.safetensors",
  name: "anime-style-v2",
  base: "z-image",
  format: "peft",
  sizeBytes: 94_000_000,
  source: "search",
  repo: "someone/Z-Image-Turbo-anime-LoRA",
  missing: false,
};

/** A third LoRA, added from this computer, whose model type its file does not say. */
const GRAIN_LORA: Lora = {
  id: "local:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
  name: "film-grain",
  base: null,
  format: "lycoris",
  sizeBytes: 38_000_000,
  source: "import",
  repo: null,
  missing: false,
};

export const LOCAL_MODELS: LocalModels = {
  ...LOCAL,
  noticeAccepted,
  // The video model was run here once (the gallery's waves clip), so its time is measured.
  installed: [
    ...LOCAL.installed.map((m) =>
      m.kind === "video" ? { ...m, fit: { ...m.fit, eta: { lowMs: 330_000, highMs: 400_000, measured: true, leftMs: null } } } : m,
    ),
    SDXL_INPAINT,
  ],
  loras: [...LOCAL.loras, ANIME_LORA, GRAIN_LORA],
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
  settingsOpen: false,
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
  lastImportMode: null,
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
// Repositories as `studio_local_repo` reads them, and the set-up plans they carry. The shapes are
// the Phase S DTOs (types.ts); the files are made up but sized as real ones are.

const zimage = LOCAL.recommended.find((e) => e.id === "z-image-turbo-q4")!;
const GIB = 1024 ** 3;
const COMMIT = "3f1c9a7d52be0e41a0c8f6d2b7e9a1c4d5f60718";
const APACHE = zimage.licence;
const OPENRAIL: Licence = {
  ...APACHE,
  id: "openrail++",
  name: "CreativeML OpenRAIL++-M",
  badge: "amber",
  commercial: "withUseRules",
  plainWords: "Commercial use is allowed, with uses the licence forbids.",
  obligations: ["Pass the use restrictions on to anyone you give the model to.", "Keep the licence with the files."],
};

const FIT: LocalFit = { ...zimage.fit, eta: null, videoOnProcessor: false };
const fitFor = (gb: number): LocalFit =>
  gb > 9
    ? { ...FIT, word: "notHere", verdict: "red", reason: "It needs more memory than this computer has." }
    : gb > 6
      ? { ...FIT, word: "slow", verdict: "yellow" }
      : { ...FIT, word: "fits", verdict: "green" };

function part(role: PartRole, repo: string, path: string, gb: number, licenceId: string, alreadyHere = false): Part {
  return { role, repo, path, sizeBytes: Math.round(gb * GIB), licenceId, alreadyHere, local: false };
}

const recipe = (id: string, name: string, kind: RecipeKind = "picture"): RecipeChoice => ({ id, name, kind });

function planOf(
  rec: RecipeChoice,
  slots: [Part, PartSource][],
  licences: Licence[],
  extra: Partial<SetupPlan> = {},
): SetupPlan {
  const parts = slots.map(([p]) => p);
  const total = parts.reduce((n, p) => n + p.sizeBytes, 0);
  const fetch = parts.filter((p) => !p.alreadyHere).reduce((n, p) => n + p.sizeBytes, 0);
  const mainGb = (parts[0]?.sizeBytes ?? 0) / GIB;
  return {
    recipe: rec,
    recipes: [rec],
    slots: slots.map(([p, source]) => ({ role: p.role, required: true, source, part: p })),
    licences,
    totalBytes: total,
    toFetchBytes: fetch,
    fit: fitFor(mainGb),
    canDownload: true,
    lowDisk: false,
    needsEngine: false,
    untested: false,
    ...extra,
  };
}

const view = (path: string, gb: number, quant: string | null, label: FileLabel): RepoFileView => ({
  path,
  sizeBytes: Math.round(gb * GIB),
  quant,
  label,
});

const Z_IMAGE_WORKS: FileLabel = {
  works: { version: "z-image", recipe: "z-image", kind: "picture", part: null, parameters: 6e9, folder: false },
};

const zImagePlan = (file: string, gb: number): SetupPlan =>
  planOf(
    recipe("z-image", "Z-Image"),
    [
      [part("diffusion", "leejet/Z-Image-Turbo-GGUF", file, gb, "apache-2.0"), "main"],
      [zimage.parts[1]!, "companion"],
      [zimage.parts[2]!, "companion"],
    ],
    [APACHE],
  );

function hit(
  repo: string,
  downloads: number,
  likes: number,
  verdict: Verdict,
  licenseLabel: string | null = "apache-2.0",
  exact = true,
): LocalSearchHit {
  return {
    repo,
    author: repo.split("/")[0]!,
    downloads,
    likes,
    updatedAt: "2026-10-30T09:12:00Z",
    gated: "none",
    licenseLabel,
    verdict,
    exact,
  };
}

const Z_IMAGE_VERDICT: Verdict = { works: { version: "z-image", kind: "picture" } };

/** What a search finds, by filter. */
const HITS: Record<SearchFilter, LocalSearchHit[]> = {
  pictures: [
    hit("leejet/Z-Image-Turbo-GGUF", 48_213, 312, Z_IMAGE_VERDICT),
    hit("jayn7/Z-Image-Turbo-GGUF", 21_904, 96, Z_IMAGE_VERDICT),
    hit("Tongyi-MAI/Z-Image-Turbo", 402_117, 2_841, { cannotRun: { reason: "sharded", version: "z-image" } }),
    hit("stabilityai/stable-diffusion-xl-base-1.0", 1_912_440, 6_804, { works: { version: "sdxl", kind: "picture" } }, "openrail++"),
    hit("Alpha-VLLM/Lumina-Image-2.0", 88_031, 541, { cannotRun: { reason: "lumina2", version: null } }),
    hit("unsloth/Z-Image-Turbo-GGUF", 9_377, 41, "unknown", "apache-2.0", false),
    hit("Comfy-Org/z_image_turbo", 156_220, 488, "unknown", "apache-2.0", false),
  ],
  video: [
    hit("QuantStack/Wan2.2-T2V-A14B-GGUF", 61_870, 233, { works: { version: "wan2", kind: "video" } }),
    hit("QuantStack/Wan2.2-TI2V-5B-GGUF", 30_144, 118, { works: { version: "wan2.2-ti2v", kind: "video" } }),
  ],
  loras: [
    hit("someone/pixel-art-xl-lora", 12_208, 187, { lora: { base: "sdxl" } }, "other"),
    hit("someone/Z-Image-Turbo-anime-LoRA", 3_208, 57, { lora: { base: "z-image" } }, "other"),
  ],
};

export function searchFor(filter: SearchFilter): LocalSearchPage {
  return { items: HITS[filter] };
}

const WAN_QUANTS: [string, number][] = [
  ["Q2_K", 5.3],
  ["Q3_K_S", 6.5],
  ["Q3_K_M", 7.2],
  ["Q4_0", 8.6],
  ["Q4_1", 9.4],
  ["Q4_K_S", 8.7],
  ["Q4_K_M", 9.7],
  ["Q5_0", 10.4],
  ["Q5_1", 11.3],
  ["Q5_K_S", 10.5],
  ["Q5_K_M", 11.4],
  ["Q6_K", 12.3],
  ["Q8_0", 15.4],
  ["F16", 28.6],
  ["BF16", 28.6],
];

function wanRepo(repo: string): LocalRepo {
  const wan = recipe("wan22-a14b", "Wan 2.2 A14B", "video");
  const label = (part: "high" | "low"): FileLabel => ({
    works: { version: "wan2", recipe: "wan22-a14b", kind: "video", part, parameters: 14e9, folder: false },
  });
  const fileViews: RepoFileView[] = [];
  const files: LocalFileChoice[] = WAN_QUANTS.map(([quant, gb], i) => {
    const high = `HighNoise/Wan2.2-T2V-A14B-HighNoise-${quant}.gguf`;
    const low = `LowNoise/Wan2.2-T2V-A14B-LowNoise-${quant}.gguf`;
    fileViews.push(view(high, gb, quant, label("high")), view(low, gb, quant, label("low")));
    const plan = planOf(
      wan,
      [
        [part("high-noise-diffusion", repo, high, gb, "apache-2.0"), "main"],
        [part("diffusion", repo, low, gb, "apache-2.0"), "sibling"],
        [part("llm", "city96/umt5-xxl-encoder-gguf", "umt5-xxl-encoder-Q5_K_M.gguf", 4.1, "apache-2.0"), "companion"],
        [part("vae", "Comfy-Org/Wan_2.2_ComfyUI_Repackaged", "split_files/vae/wan_2.1_vae.safetensors", 0.25, "apache-2.0"), "companion"],
      ],
      [APACHE],
    );
    return { file: high, quant, label: label("high"), plan, best: i === 3 };
  });
  return {
    repo,
    commit: COMMIT,
    author: "QuantStack",
    summary: "GGUF quantisations of Wan 2.2 A14B text-to-video: a high-noise and a low-noise model, run one after the other.",
    verdict: { works: { version: "wan2", kind: "video" } },
    files,
    loras: [],
    fileViews,
    licence: APACHE,
    licences: [APACHE],
    labelMismatch: null,
    gate: { mode: "none", collects: [] },
    readFiles: fileViews.length,
    unreadFiles: 0,
  };
}

export function repoFor(repo: string): LocalRepo {
  const base = {
    repo,
    commit: COMMIT,
    author: repo.split("/")[0]!,
    licence: APACHE as Licence | null,
    licences: [APACHE],
    labelMismatch: null,
    gate: { mode: "none", collects: [] } as Gate,
    loras: [] as RepoFileView[],
    unreadFiles: 0,
  };
  if (repo.includes("Wan2.2")) return wanRepo(repo);
  if (repo.endsWith("-GGUF")) {
    const quants: [string, string, number][] = [
      ["z_image_turbo-Q4_K.gguf", "Q4_K", 3.6],
      ["z_image_turbo-Q5_K_M.gguf", "Q5_K_M", 4.3],
      ["z_image_turbo-Q8_0.gguf", "Q8_0", 6.7],
    ];
    const fileViews = quants.map(([file, quant, gb]) => view(file, gb, quant, Z_IMAGE_WORKS));
    return {
      ...base,
      summary: "Z-Image Turbo as GGUF files: photo-real pictures in 8 steps.",
      verdict: Z_IMAGE_VERDICT,
      files: quants.map(([file, quant, gb], i) => ({
        file,
        quant,
        label: Z_IMAGE_WORKS,
        plan: zImagePlan(file, gb),
        best: i === 0,
      })),
      fileViews,
      readFiles: fileViews.length,
    };
  }
  if (repo.includes("stable-diffusion-xl")) {
    const label: FileLabel = {
      works: { version: "sdxl", recipe: "sdxl-checkpoint", kind: "picture", part: null, parameters: 3.5e9, folder: false },
    };
    const file = "sd_xl_base_1.0.safetensors";
    // Two recipes its version runs, so the panel offers Model type.
    const sdxl = recipe("sdxl-checkpoint", "SDXL");
    const plan = planOf(sdxl, [[part("model", repo, file, 6.46, "openrail++"), "main"]], [OPENRAIL], {
      recipes: [sdxl, recipe("sdxl-turbo", "SDXL Turbo")],
    });
    return {
      ...base,
      summary: "SDXL 1.0 base: a general picture model, 1024 px.",
      verdict: { works: { version: "sdxl", kind: "picture" } },
      files: [{ file, quant: "fp16", label, plan, best: true }],
      licence: OPENRAIL,
      licences: [OPENRAIL],
      fileViews: [view(file, 6.46, "fp16", label)],
      readFiles: 1,
    };
  }
  if (repo.includes("Lumina")) {
    const cannot: FileLabel = { cannotRun: { reason: "lumina2", version: null } };
    return {
      ...base,
      summary: "Lumina-Image 2.0: a 2.6B picture model.",
      verdict: { cannotRun: { reason: "lumina2", version: null } },
      files: [],
      fileViews: [view("consolidated.00-of-01.pth", 10.4, null, cannot), view("ae.safetensors", 0.32, null, { extra: { role: "vae" } })],
      readFiles: 2,
    };
  }
  if (/lora/i.test(repo)) {
    const lbase: SdVersion = repo.includes("Z-Image") ? "z-image" : "sdxl";
    const lora: FileLabel = { lora: { base: lbase } };
    const files = [view("pixel-art-xl-v1.1.safetensors", 0.17, null, lora), view("pixel-art-xl-lcm.safetensors", 0.17, null, lora)];
    return {
      ...base,
      summary: "A LoRA for pixel-art pictures.",
      verdict: { lora: { base: lbase } },
      files: [],
      loras: files,
      fileViews: files,
      licence: null,
      licences: [],
      readFiles: files.length,
    };
  }
  // Not supported yet: the original release, in shards.
  const cannot: FileLabel = { cannotRun: { reason: "sharded", version: "z-image" } };
  const fileViews = [
    view("z_image_turbo_bf16.safetensors", 12.3, null, cannot),
    view("split_files/vae/ae.safetensors", 0.31, null, { extra: { role: "vae" } }),
    view("split_files/text_encoders/qwen_3_4b.safetensors", 8.0, null, { extra: { role: "llm" } }),
  ];
  return {
    ...base,
    summary: "The original Z-Image Turbo release.",
    verdict: { cannotRun: { reason: "sharded", version: "z-image" } },
    files: [],
    fileViews,
    readFiles: fileViews.length,
  };
}

const RECIPES: RecipeChoice[] = [
  recipe("sdxl-checkpoint", "SDXL"),
  recipe("flux2-klein-4b", "FLUX.2 klein 4B"),
  recipe("z-image", "Z-Image"),
  recipe("wan22-ti2v-5b", "Wan 2.2 5B", "video"),
];

/** An installed model's files, for *Set it up yourself*. */
export function filesOf(installed: InstalledLocal[], genModelId: string, pickedRecipe: string | null = null): ModelFiles {
  const model = installed.find((m) => m.id === genModelId) ?? installed[0]!;
  const rec = pickedRecipe ? (RECIPES.find((r) => r.id === pickedRecipe) ?? model.recipe) : model.recipe;
  const main: Part = {
    ...part("model", "", `/home/me/models/${model.name}.safetensors`, model.sizeBytes / GIB, "", true),
    local: model.inPlace,
  };
  // An SDXL set-up (Task U8's Files screen): the usual decoder, and its two text encoders
  // inside the model file.
  const slots: PlannedSlot[] =
    rec?.id === "sdxl-checkpoint"
      ? [
          { role: "model", required: true, source: "main", part: main },
          {
            role: "vae",
            required: true,
            source: "companion",
            part: part("vae", "madebyollin/sdxl-vae-fp16-fix", "sdxl_vae.safetensors", 0.31, "mit", true),
          },
          { role: "clip-l", required: false, source: "embedded", part: null },
          { role: "clip-g", required: false, source: "embedded", part: null },
        ]
      : [
          { role: "model", required: true, source: "main", part: main },
          rec
            ? { role: "vae", required: false, source: "embedded", part: null }
            : { role: "vae", required: true, source: "missing", part: null },
        ];
  return {
    genModelId: model.id,
    recipe: rec,
    recipes: model.untested || !rec ? RECIPES : [rec],
    slots,
    untested: model.untested || (pickedRecipe !== null && pickedRecipe !== model.recipe?.id),
    canRun: slots.every((s) => !s.required || s.part !== null),
  };
}

/** The plan a download, fit or set-up plan is asked for: of a catalog entry, a searched file, an
 *  installed model, or a file from this computer. */
export function planFor(installed: InstalledLocal[], source: LocalSource, choice: SetupChoice | null): SetupPlan {
  let plan: SetupPlan;
  if ("catalog" in source) {
    const entry = LOCAL.recommended.find((e) => e.id === source.catalog.entryId) ?? zimage;
    plan = planOf(
      recipe("z-image", "Z-Image"),
      entry.parts.map((p, i): [Part, PartSource] => [p, i === 0 ? "main" : "companion"]),
      entry.licences,
      { fit: entry.fit },
    );
  } else if ("search" in source) {
    const { repo, file } = source.search;
    plan = repoFor(repo).files.find((f) => f.file === file)?.plan ?? zImagePlan(file, 3.6);
  } else if ("installed" in source) {
    const files = filesOf(installed, source.installed.genModelId);
    plan = planOf(
      files.recipe ?? RECIPES[0]!,
      files.slots.filter((s) => s.part).map((s): [Part, PartSource] => [s.part!, s.source]),
      [],
      { recipes: files.recipes, fit: fitFor(6.46), toFetchBytes: 0 },
    );
  } else {
    // A model file from this computer: the file itself, and SDXL's usual decoder to download.
    const main: Part = { ...part("model", "", source.disk.path, 6.46, "", true), local: true };
    const vae = part("vae", "madebyollin/sdxl-vae-fp16-fix", "sdxl_vae.safetensors", 0.31, "mit");
    plan = planOf(recipe("sdxl-checkpoint", "SDXL"), [[main, "main"], [vae, "companion"]], [], { fit: fitFor(6.46) });
  }
  const picked = choice?.recipe ? RECIPES.find((r) => r.id === choice.recipe) : undefined;
  return picked ? { ...plan, recipe: picked, recipes: RECIPES, untested: true } : plan;
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
