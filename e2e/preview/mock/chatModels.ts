// Chat models for Models -> Browse, Installed and Compare in the preview (`?cantopen=1`): the
// owner's repository with its real file list and checksums (read from Hugging Face on
// 2026-11-27), carrying a kind this version does not know ("inkling3_loop", made up: the real
// one, "inkling2_loop", is known since the llama.cpp bump), and a repository that opens.
// Generated once; the memory verdicts are made up (the page does not show them for a model
// that cannot open).
import type {
  CompareFacts,
  FitResult,
  InstalledModel,
  RepoSummary,
  RepoView,
  SearchPage,
  Verdict,
} from "../../../src/lib/types";

export const CANT_OPEN_REPO = "DavidAU/Inkling-2-Mini-26B-Heretic-Uncensored-Turbo-Brilliance-50X-V2-AGI-NEO-GGUF";
export const OPENS_REPO = "unsloth/Qwen3-8B-GGUF";
/** The kind this preview's version does not know. */
export const NEW_KIND = "inkling3_loop";

const PROJECTOR = { path: "mmproj-F16.gguf", size: 639348160, sha256: "3bff75ffeb2d6e0371ba7bd42e30d26a7d98aea681fa44b03b5a20891cfa2c5f" };

const OWNERS_FILES = [
  { label: "Q8_0", path: "Inkling-2-Mini-26B-Heretic-Uncensored-Turbo-Brilliance-50X-V2-AGI-NEO-Q8_0.gguf", size: 27820341856, sha256: "1676ea9e579ee3ce2d07063288c515bc85dadc901451d985c661a3545df98f82" },
  { label: "Q6_K", path: "Inkling-2-Mini-26B-Heretic-Uncensored-Turbo-Brilliance-50X-V2-AGI-NEO-Q6_K.gguf", size: 23490083680, sha256: "179ad23f405b433b8d70770983d563a0576fc32e0b74f19e74e5d5bbd56acf36" },
  { label: "Q5_K_M", path: "Inkling-2-Mini-26B-Heretic-Uncensored-Turbo-Brilliance-50X-V2-AGI-NEO-Q5_K_M.gguf", size: 19906679008, sha256: "1c6fd88579247ff64e70ccb6b9e37680a8f44ed1e21ae84980d9a2b70fb6c068" },
  { label: "Q5_1", path: "Inkling-2-Mini-26B-Heretic-Uncensored-Turbo-Brilliance-50X-V2-AGI-NEO-Q5_1.gguf", size: 19717437664, sha256: "93e6a33064fdcd374ab730e7023b37b736eedbbd2be28dbb062e909d402da766" },
  { label: "Q5_K_S", path: "Inkling-2-Mini-26B-Heretic-Uncensored-Turbo-Brilliance-50X-V2-AGI-NEO-Q5_K_S.gguf", size: 18628899040, sha256: "1c1dfe6b8ed67a8fc16f020b4347fb63ede5d97938ff3e37a5ff71765dd1a016" },
  { label: "Q4_K_M", path: "Inkling-2-Mini-26B-Heretic-Uncensored-Turbo-Brilliance-50X-V2-AGI-NEO-Q4_K_M.gguf", size: 17478336736, sha256: "6f4c35609e0095a13688abcd0d93298bab9d930c44686fba730a25b584ffd287" },
  { label: "IQ4_NL", path: "Inkling-2-Mini-26B-Heretic-Uncensored-Turbo-Brilliance-50X-V2-AGI-NEO-IQ4_NL.gguf", size: 15305203936, sha256: "6683df47a5e93c48063f9d571e0cc1797d859c5690488e2a80212a78037a36bb" },
  { label: "Q4_0", path: "Inkling-2-Mini-26B-Heretic-Uncensored-Turbo-Brilliance-50X-V2-AGI-NEO-Q4_0.gguf", size: 14922997984, sha256: "c24445027d7cd89ec3eb7215c719e663a7953f4330d9bfc059ec4c98aa3fcb91" },
  { label: "IQ4_XS", path: "Inkling-2-Mini-26B-Heretic-Uncensored-Turbo-Brilliance-50X-V2-AGI-NEO-IQ4_XS.gguf", size: 14760934624, sha256: "f0abab87cbeef540b593ec0485098f9ca151453a66f3b7f5c5f611772716cb07" },
  { label: "Q3_K_M", path: "Inkling-2-Mini-26B-Heretic-Uncensored-Turbo-Brilliance-50X-V2-AGI-NEO-Q3_K_M.gguf", size: 13723441504, sha256: "b59a730ec0412f75c542309469ea69baf890f8754ec812fa5ec937b5ba6fefb6" },
  { label: "Q2_K", path: "Inkling-2-Mini-26B-Heretic-Uncensored-Turbo-Brilliance-50X-V2-AGI-NEO-Q2_K.gguf", size: 10863981280, sha256: "c5d986b85618783a4d1e18013e0e49874ae517315df0de8f0714cca8975db5a4" },
];

const GIB = 1024 ** 3;

export function fitOf(verdict: Verdict, needed: number): FitResult {
  return {
    verdict,
    neededBytes: needed,
    availableBytes: 14.9 * GIB,
    suggestedCtx: null,
    partialOffload: verdict === "yellow",
    device: "graphicsCard",
    computerBytes: verdict === "yellow" ? 6 * GIB : 0,
    computerFreeBytes: verdict === "yellow" ? 18 * GIB : 0,
    computerShort: false,
    sentence: "",
  };
}

function verdictFor(size: number): Verdict {
  return size > 26e9 ? "red" : size > 12e9 ? "yellow" : "green";
}

const COMMIT = "d41db62659b4fec755bb4acc27903508221914a9";

/** The owner's repository as `hub_repo` answers for a kind this version does not know. */
export function cantOpenView(): RepoView {
  return {
    id: CANT_OPEN_REPO,
    commit: COMMIT,
    gated: "no",
    license: "apache-2.0",
    architecture: NEW_KIND,
    architectureSupported: false,
    contextLength: 1_048_576,
    parameters: 26_070_385_152,
    templateTools: true,
    templateThinking: true,
    projector: PROJECTOR,
    variants: OWNERS_FILES.map((f) => ({
      label: f.label,
      files: [{ path: f.path, size: f.size, sha256: f.sha256 }],
      totalSize: f.size,
      bits: null,
      // What the page of 0.2.1 showed under its red box; the page now hides it.
      fit: fitOf(verdictFor(f.size), f.size * 1.08),
      installed: false,
      hasChecksum: true,
    })),
    incomplete: [],
    card: "# Inkling 2 Mini 26B\n\nA made-up model card for the preview.",
    fitContext: 8192,
  };
}

/** A repository this version opens, for the contrast (a small Qwen model, real sizes). */
export function opensView(): RepoView {
  const sizes: [string, number][] = [
    ["Q3_K_M", 4_120_000_000],
    ["Q4_K_M", 5_027_783_488],
    ["Q5_K_M", 5_850_000_000],
    ["Q6_K", 6_730_000_000],
    ["Q8_0", 8_710_000_000],
  ];
  return {
    id: OPENS_REPO,
    commit: "a".repeat(40),
    gated: "no",
    license: "apache-2.0",
    architecture: "qwen3",
    architectureSupported: true,
    contextLength: 40960,
    parameters: 8_190_000_000,
    templateTools: true,
    templateThinking: true,
    projector: null,
    variants: sizes.map(([label, size]) => ({
      label,
      files: [{ path: `Qwen3-8B-${label}.gguf`, size, sha256: "0".repeat(64) }],
      totalSize: size,
      bits: null,
      fit: fitOf("green", size * 1.1),
      installed: false,
      hasChecksum: true,
    })),
    incomplete: [],
    card: null,
    fitContext: 8192,
  };
}

export function repoViewFor(repo: string): RepoView {
  return repo === CANT_OPEN_REPO ? cantOpenView() : { ...opensView(), id: repo };
}

function hit(id: string, downloads: number, patch: Partial<RepoSummary> = {}): RepoSummary {
  return {
    id,
    downloads,
    likes: 0,
    gated: "no",
    license: "apache-2.0",
    lastModified: "2026-11-24T02:39:11.000Z",
    pipelineTag: "text-generation",
    ...patch,
  };
}

/** `hub_search`: the owner's repository (the listing names the new kind) among ones that open. */
export function searchPage(): SearchPage {
  return {
    items: [
      hit("unsloth/Qwen3-8B-GGUF", 4_812_330),
      hit("bartowski/gemma-3-12b-it-GGUF", 1_204_551, { license: "gemma" }),
      hit(CANT_OPEN_REPO, 39_177, { pipelineTag: "image-text-to-text", unknownArchitecture: NEW_KIND }),
      hit("ggml-org/gpt-oss-20b-GGUF", 902_118),
    ],
    next: null,
  };
}

/** An installed copy of the owner's Q6_K file, with the image file its repository offers. */
export const INSTALLED_CANT_OPEN: InstalledModel = {
  id: 3,
  path: "/home/me/models/DavidAU/Inkling-2-Mini-26B-Heretic-Uncensored-Turbo-Brilliance-50X-V2-AGI-NEO-GGUF/Inkling-2-Mini-26B-Heretic-Uncensored-Turbo-Brilliance-50X-V2-AGI-NEO-Q6_K.gguf",
  repo: CANT_OPEN_REPO,
  fileName: "Inkling-2-Mini-26B-Heretic-Uncensored-Turbo-Brilliance-50X-V2-AGI-NEO-Q6_K.gguf",
  sizeBytes: 23490083680,
  source: "download",
  projectorPath: null,
  summary: {
    architecture: NEW_KIND,
    quantization: "Q6_K",
    sizeLabel: "26B",
    contextLength: 1_048_576,
    templateTools: true,
    templateThinking: true,
    isMoe: true,
  },
  architectureSupported: false,
  imageFileOffer: { kind: "available", file: PROJECTOR.path, sizeBytes: PROJECTOR.size },
};

/** `compare_facts` for the preview's two models (the owner's, and the one that opens). */
export function compareFacts(item: Record<string, string>): CompareFacts {
  const cant = item.repo === CANT_OPEN_REPO;
  const file = cant ? (OWNERS_FILES.find((f) => f.path === item.file) ?? OWNERS_FILES[1]!) : { path: "Qwen3-8B-Q4_K_M.gguf", size: 5_027_783_488, label: "Q4_K_M" };
  const size = file.size;
  return {
    item: item as unknown as CompareFacts["item"],
    title: String(item.repo),
    file: file.path,
    quant: file.label,
    sizeBytes: size,
    installedPath: null,
    ctx: 8192,
    fit: fitOf(cant ? "yellow" : "green", size * 1.08),
    catalog: null,
    baseCatalog: null,
    hub: {
      repo: String(item.repo),
      author: String(item.repo).split("/")[0]!,
      pipelineTag: cant ? "image-text-to-text" : "text-generation",
      baseModel: null,
      languages: ["en"],
      license: { name: "apache-2.0", url: null },
      tags: ["gguf", "conversational"],
      downloads: cant ? 39_177 : 4_812_330,
      likes: cant ? 44 : 310,
      lastModified: "2026-11-24T02:39:11Z",
      gated: false,
      cardUrl: `https://huggingface.co/${item.repo}`,
    },
    fileFacts: {
      architecture: cant ? NEW_KIND : "qwen3",
      engineKnows: !cant,
      parameters: cant ? 26_070_385_152 : 8_190_000_000,
      contextLength: cant ? 1_048_576 : 40960,
      tools: true,
      thinking: true,
      vision: cant,
      moe: cant,
      quantization: file.label,
      expertCount: cant ? 128 : null,
      expertUsedCount: cant ? 8 : null,
      activeParameters: cant ? 3_800_000_000 : null,
    },
    results: [],
    problem: null,
  };
}
