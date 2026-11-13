// Types shared with the Rust side (src-tauri/src/dto.rs). Field names are camelCase
// on both sides. Keep the two files in step.

export interface ErrorInfo {
  code: string;
  message: string;
  nextStep: string;
  details: string;
}

export interface ChatSummary {
  id: number;
  title: string;
  updatedAt: number;
  snippet: string | null;
}

export interface Chat {
  id: number;
  title: string;
  modelRef: string | null;
  systemPrompt: string | null;
  leafMessageId: number | null;
  createdAt: number;
  updatedAt: number;
  /** Thinking is on for this chat (when the model can think). */
  thinking: boolean;
  /** The folder relative file paths start from. Null: the first approved folder. */
  workingFolder: string | null;
}

export type Role = "system" | "user" | "assistant" | "tool";
export type MessageStatus = "streaming" | "complete" | "stopped" | "error";

export interface MessageDto {
  id: number;
  parentId: number | null;
  role: Role;
  text: string;
  reasoning: string | null;
  status: MessageStatus;
  modelRef: string | null;
  promptTokens: number | null;
  completionTokens: number | null;
  tokensPerSecond: number | null;
  error: ErrorInfo | null;
  createdAt: number;
  /** All messages that share this message's parent, including itself, oldest first. */
  siblingIds: number[];
  /** Attached images (load with getAttachment). */
  imageIds: number[];
  /** Attached text and PDF files. */
  documents: { id: number; name: string }[];
  /** Tool calls the model made in this message. */
  toolCalls: ToolCallDto[];
  /** For a tool message: the result. */
  toolResult: ToolResultDto | null;
  /** For a user message: the files and definitions it mentioned. */
  mentions?: MentionRef[];
}

export interface ToolCallDto {
  id: string;
  name: string;
  arguments: Record<string, unknown>;
  /** Recovered from the model's text rather than a native tool call. */
  recovered: boolean;
}

export interface ToolResultDto {
  callId: string;
  name: string;
  /** ok, error, denied, cancelled, refused, ... */
  status: string;
  text: string;
  imageCount: number;
}

/** A text or PDF file picked or dropped into the composer. */
export interface DocumentInput {
  fileName: string;
  /** Base64 of the bytes. */
  data: string;
}

/** A mention chosen in the picker, as sent with a message (`@path` or `@path#name`). */
export interface MentionInput {
  /** The file, absolute. */
  path: string;
  symbol?: string;
  container?: string;
  kind?: string;
  /** Where the definition was when it was picked. */
  line?: number;
}

/** One entry of the mention picker. */
export interface MentionHit {
  /** "file", or the kind of the definition ("function", "class", ...). */
  kind: string;
  /** The text of the mention: "src/lib.rs" or "src/lib.rs#parse_args". */
  label: string;
  path: string;
  symbol?: string;
  container?: string;
  line?: number;
  endLine?: number;
}

export interface MentionSearch {
  hits: MentionHit[];
  /** Still indexing: asking again may find more. */
  indexing: boolean;
  files: number;
  folders: number;
}

/** A mention stored with a sent message. */
export interface MentionRef {
  attachmentId: number;
  label: string;
  kind: string;
  path: string;
  symbol?: string;
  container?: string;
  startLine: number;
  endLine: number;
  truncated: boolean;
}

/** What the app would send for one mention, or why it cannot. */
export interface MentionCheck {
  label: string;
  kind: string;
  startLine: number;
  endLine: number;
  totalLines: number;
  truncated: boolean;
  error?: string;
}

/** An image picked, pasted or dropped into the composer. */
export interface ImageInput {
  mime: string;
  fileName: string;
  /** Base64 of the bytes, without a data: prefix. */
  data: string;
}

export interface ChatDetail {
  chat: Chat;
  messages: MessageDto[];
  /** [0.0.5 memory] The summary the model gets instead of the earlier messages of the
   *  shown branch (null when there is none; absent in older test fixtures). */
  summary?: EarlierSummary | null;
}

/** [0.0.5 memory] A summary of earlier messages. Model output: shown as plain text only. */
export interface EarlierSummary {
  /** The newest message it stands for; every message above it on the branch is covered. */
  lastMessageId: number;
  text: string;
}

export type RuntimeState = "stopped" | "starting" | "ready" | "stopping" | "failed";

export interface RuntimeStatus {
  state: RuntimeState;
  backend: string | null;
  device: string | null;
  modelPath: string | null;
  modelName: string | null;
  progress: number | null;
  stage: string | null;
  nCtx: number | null;
  supportsTools: boolean;
  preserveReasoning: boolean;
  /** The model's chat template can think. */
  thinking: boolean;
  /** Tokens/s of the short generation timed after the load. */
  speedTps: number | null;
  /** A GPU backend generated far below expectation ("loaded but slow"). */
  slow: boolean;
  vision: boolean;
  offloadedLayers: number | null;
  totalLayers: number | null;
  error: ErrorInfo | null;
  /** 0.0.5: where the load is, typed (plain words in words.ts). `stage` keeps the old text. */
  loadStage: LoadStage | null;
  /** 0.0.5: what this load had to change to start (step-downs, retries), in order. */
  adjustments: LoadStage[];
  /** 0.0.5: why the loaded context is its size, for the ready line. Null until WP1b fills it. */
  ctxSource: ContextSource | null;
  /** 0.0.5: the loaded copy uses older settings than the saved ones. */
  settingsChanged: boolean;
  /**
   * 0.0.5: a reload, a load of another model or an unload waits for the model to be free
   * (called with `whenIdle`; "Restarts when the reply ends · Cancel"). The other fields
   * still describe the model loaded now.
   */
  reloadWaiting: boolean;
  /** 0.0.5: the model the user unloaded ("Unloaded to free memory · Load"); null after any load. */
  unloadedPath: string | null;
  /** 0.0.6 item 6: where the loaded model went (the ready line's memory report); null until ready. */
  placement: Placement | null;
}

export interface DeviceInfo {
  name: string;
  description: string;
  totalMib: number;
  freeMib: number;
}

export interface BackendInfo {
  kind: string;
  source: string;
  build: string;
  available: boolean;
  devices: DeviceInfo[];
  error: string | null;
}

export type ProviderKind = "ollama" | "lmstudio" | "vllm" | "generic";

export interface Provider {
  id: number;
  name: string;
  kind: ProviderKind;
  baseUrl: string;
  hasApiKey: boolean;
  enabled: boolean;
}

export interface ModelOption {
  ref: string;
  label: string;
  source: "local" | "provider";
  contextLength: number | null;
  /** [0.0.6 item 8] The end of a local file's folder ("Downloads\models"), only when another
   *  file in the list has the same name. */
  hint?: string;
  /** [0.0.6 item 8] A local file used recently; installed models never used are left without
   *  it. The list starts with these, then the endpoints' models, then the other installed ones. */
  recent?: boolean;
}

/**
 * The defaults for all models (src-tauri/src/dto.rs). The 0.0.5 fields are always sent by
 * the backend; a value without them (the 0.0.4 shape) is still accepted by the commands.
 */
export interface LocalModelSettings {
  ctx: number;
  backend: string | null;
  device: string | null;
  /** 0.0.5: "How much it can remember" is Automatic. */
  contextAuto: boolean;
  gpu: GpuUse;
  saveMemory: boolean;
  keepInMemory: boolean;
  thinkingLimit: ThinkingLimit;
  /** Response only: show the one-time upgrade note with this size (D2). Never stored. */
  upgradeNote?: number;
}

/** Events streamed while an agent turn runs (src-tauri/src/turns.rs). */
export type TurnStreamEvent =
  | { type: "started"; userMessageId: number | null }
  | { type: "step"; step: number }
  | { type: "content"; text: string }
  | { type: "reasoning"; text: string }
  | { type: "discarded" }
  | { type: "messageSaved"; messageId: number; role: Role }
  | { type: "toolStarted"; callId: string; name: string; arguments: Record<string, unknown>; recovered: boolean }
  | { type: "toolProgress"; callId: string; text: string }
  | {
      type: "toolFinished";
      callId: string;
      name: string;
      status: string;
      text: string;
      decision: string;
      durationMs: number;
      ui: unknown;
    }
  | {
      type: "usage";
      promptTokens: number;
      completionTokens: number;
      tokensPerSecond: number | null;
      contextUsed: number | null;
      nCtx: number | null;
    }
  | { type: "note"; text: string }
  | { type: "compacted"; stage: number }
  | { type: "finished"; status: string; message: string | null };

export interface DiagnosticsReport {
  text: string;
  savedTo: string | null;
}

// ---- Model manager (M2) -----------------------------------------------------

export type Gated = "no" | "auto" | "manual";
export type HubSort = "downloads" | "trendingScore" | "lastModified" | "likes";

export interface RepoSummary {
  id: string;
  downloads: number;
  likes: number;
  gated: Gated;
  license: string | null;
  lastModified: string | null;
  pipelineTag: string | null;
}

export interface SearchPage {
  items: RepoSummary[];
  next: string | null;
}

export interface RepoFile {
  path: string;
  size: number;
  sha256: string | null;
}

export type Verdict = "green" | "yellow" | "red";

/** Where a fit estimate puts the model (0.0.6, compare-fit-words). */
export type FitDevice = "graphicsCard" | "appleGraphics" | "processor";

export interface FitResult {
  verdict: Verdict;
  /** What the model needs in the memory that holds it. */
  neededBytes: number;
  /** Free memory there: the graphics card's, what Apple graphics may use, or the computer's. */
  availableBytes: number;
  suggestedCtx: number | null;
  partialOffload: boolean;
  device: FitDevice;
  /** The backend's 0.0.4 sentence; the screens word the figures with `fitLine` (words.ts). */
  sentence: string;
}

export interface VariantView {
  label: string;
  files: RepoFile[];
  totalSize: number;
  bits: number | null;
  fit: FitResult | null;
  installed: boolean;
  hasChecksum: boolean;
}

export interface RepoView {
  id: string;
  commit: string;
  gated: Gated;
  license: string | null;
  architecture: string | null;
  /** False when the bundled llama.cpp cannot load this architecture; null if unknown. */
  architectureSupported: boolean | null;
  contextLength: number | null;
  parameters: number | null;
  templateTools: boolean;
  templateThinking: boolean;
  projector: RepoFile | null;
  variants: VariantView[];
  incomplete: string[];
  /** Model card as plain text (untrusted; never rendered as HTML). */
  card: string | null;
  fitContext: number;
}

export type DownloadStatus =
  | "queued"
  | "downloading"
  | "paused"
  | "verifying"
  | "done"
  | "error"
  | "cancelled";

export interface DownloadGroup {
  id: string;
  repo: string;
  label: string;
  fileCount: number;
  bytesDone: number;
  bytesTotal: number;
  status: DownloadStatus;
  error: ErrorInfo | null;
  bytesPerSecond: number | null;
  /** A file without a published checksum is part of this download. */
  unverified: boolean;
}

export interface ModelSummaryView {
  architecture: string;
  quantization: string | null;
  sizeLabel: string | null;
  contextLength: number | null;
  templateTools: boolean;
  templateThinking: boolean;
  isMoe: boolean;
}

export interface InstalledModel {
  id: number;
  path: string;
  repo: string | null;
  fileName: string;
  sizeBytes: number;
  source: "download" | "import";
  projectorPath: string | null;
  summary: ModelSummaryView | null;
  architectureSupported: boolean | null;
  /** [0.0.6 item 8] The path the model loaded now was loaded with, when it is this row's file
   *  under any spelling (a link, a mapped drive). Sent only then. */
  loadedAs?: string;
}

export interface HardwareInfo {
  target: "gpu" | "apple" | "cpu";
  gpuName: string | null;
  gpuFreeBytes: number | null;
  ramTotalBytes: number | null;
  ramAvailableBytes: number | null;
  /** Memory the fit check compares against. */
  usableBytes: number;
  backend: string | null;
  /** A Mac with an Intel processor, where models run on the CPU. */
  intelMac: boolean;
}

export interface CatalogSuggestion {
  id: string;
  name: string;
  repo: string;
  quant: string;
  capabilities: string[];
  license: string;
  totalBytes: number;
  memoryNeeded: number;
  recommendedCtx: number;
  tierGb: number;
  fits: boolean;
  tested: boolean;
  installed: boolean;
}

export interface FirstRunInfo {
  done: boolean;
  hardware: HardwareInfo;
  pick: string | null;
  suggestions: CatalogSuggestion[];
  catalogSequence: number;
  refreshDisabledReason: string | null;
}

export interface DiskUsage {
  modelsDir: string;
  totalBytes: number;
}

export interface PackInfo {
  id: string;
  title: string;
  backend: string;
  requirements: string;
  licenseName: string;
  licenseUrl: string;
  downloadBytes: number;
  state: "notInstalled" | "partial" | "downloading" | "installed";
  bytesDone: number;
  available: boolean;
  reason: string | null;
}

export interface PackProgress {
  id: string;
  done: number;
  total: number;
  unpacking: boolean;
  finished: boolean;
  error: ErrorInfo | null;
}

// ---- Tools, approvals, audit (M3) ------------------------------------------

export interface ApprovalRequest {
  /** Random id created in Rust; the decision is sent back with it. */
  id: string;
  chatId: number;
  callId: string;
  server: string;
  tool: string;
  summary: string;
  preview: string;
  card: "onceOnly" | "grantable";
  reason: "normal" | "alwaysAsk" | "recoveredFromText" | "untrustedContentThenOutbound" | "stillAsksInAllowAll";
  /** Why the action asks whatever the chat's approval mode; null when Allow all would let it through. */
  keepAsking?: KeepAsking | null;
}

/** The kinds of action that still ask while a chat is in Allow all. */
export type KeepAsking =
  | "delete"
  | "destroy"
  | "sensitive"
  | "publish"
  | "protectedApp"
  | "markedDestructive"
  | "hidden";

export interface ApprovedFolder {
  id: number;
  path: string;
  chatId: number | null;
}

export interface AuditRow {
  id: number;
  ts: number;
  chat_id: number | null;
  server: string;
  tool: string;
  arguments: unknown;
  preview: string;
  risk: string;
  decision: string;
  decided_by: string;
  note: string | null;
  result_status: string | null;
  result_summary: string | null;
  duration_ms: number | null;
}

export interface AuditFilter {
  chatId?: number | null;
  tool?: string | null;
  decision?: string | null;
  since?: number | null;
  limit?: number | null;
}

export interface HotkeyStatus {
  accelerator: string;
  registered: boolean;
  error: string | null;
}

// ---- Tools and MCP servers (M3) ----------------------------------------------

export interface ToolDto {
  exposed: string;
  server: string;
  original: string;
  description: string;
  builtin: boolean;
  enabledInChat: boolean;
  /** External tools: switched on in the server's list, and this version reviewed. */
  available: boolean;
  /** External tools: changed since it was last reviewed. */
  changed: boolean;
  allowAnyArgs: boolean;
  alwaysAsk: boolean;
  destructive: boolean;
  ready: boolean;
  schemaError: string | null;
  tokenEstimate: number;
  /** Comes along with any other tool (find_tools, call_tool, read_output, update_task): not a switch. */
  companion: boolean;
  /** On, and in the list the model gets whatever the next message says. On but not shown: "on request". */
  shown: boolean;
}

/** The `toolSelection` setting: choose the tools for each task, and how many a turn shows at most. */
export interface ToolSelection {
  auto: boolean;
  max: number;
}

export type ServerState =
  | { type: "starting" }
  | { type: "ready" }
  | { type: "failed"; reason: string }
  | { type: "stopped" };

export interface McpServerDto {
  id: number | null;
  name: string;
  kind: "builtin" | "stdio" | "http";
  state: ServerState;
  enabled: boolean;
  builtin: boolean;
  summary: string;
  protocolVersion: string | null;
  stderrTail: string[];
  toolCount: number;
}

export interface McpImportPreview {
  name: string;
  kind: string;
  summary: string;
  exists: boolean;
  findings: { what: string; name: string }[];
  envKeys: string[];
  headerKeys: string[];
}

// ---- Skills (M5) ---------------------------------------------------------------

export interface SkillDto {
  name: string;
  description: string;
  dir: string;
  enabled: boolean;
  /** Read from the shared ~/.agents/skills folder (not editable here). */
  shared: boolean;
  license: string | null;
  compatibility: string | null;
  allowedTools: string[];
  warnings: string[];
}

export interface SkillList {
  skills: SkillDto[];
  problems: { path: string; error: string }[];
  sharedEnabled: boolean;
  sharedPath: string | null;
  folder: string | null;
}

// ---- Updates (M8) ----------------------------------------------------------------

export interface UpdateStatus {
  configured: boolean;
  autoCheck: boolean;
  currentVersion: string;
  /** An update found by the automatic or manual check since the app started. */
  available: UpdateInfo | null;
}

export interface UpdateInfo {
  version: string;
  date: string | null;
  notes: string | null;
}

// ---- Browser tool (M6) -------------------------------------------------------------

export interface BrowserSettings {
  path: string | null;
  loopbackPort: boolean;
  allowPrivateNetworks: boolean;
  strictSites: string[];
}

export interface BrowserStatus {
  settings: BrowserSettings;
  found: string | null;
  /** The name of the browser in use, e.g. "Google Chrome". */
  kind: string | null;
  unavailable: string | null;
  running: boolean;
  /** The browser the app can download for the agent. */
  own: OwnBrowserStatus;
}

/** The agent's own browser (a pinned Chrome for Testing build). */
export interface OwnBrowserStatus {
  /** True where the download is offered (decided by the backend; macOS only). */
  offered: boolean;
  state: "notInstalled" | "partial" | "downloading" | "installed";
  /** The installed version, when installed. */
  version: string | null;
  /** The version a download would install. */
  pinnedVersion: string;
  downloadBytes: number;
  bytesDone: number;
  /** True when the browser in use is this one. */
  inUse: boolean;
}

export interface AgentBrowserProgress {
  done: number;
  total: number;
  unpacking: boolean;
  finished: boolean;
  error: ErrorInfo | null;
}

// ---- Computer use (M7) -------------------------------------------------------------

export type Convention = "pixels" | "grid1000" | "box1000" | "box1000Yx";

export interface ComputerStatus {
  available: boolean;
  screenRecording: boolean;
  accessibility: boolean;
  screenRecordingUrl: string;
  accessibilityUrl: string;
  /** How the asked-about model writes positions (null when no model was named). */
  profile: ComputerProfileStatus | null;
}

export interface ComputerProfileStatus {
  /** What computer use runs with for this model. */
  convention: Convention;
  /** It is the automatic pick; false when the user chose one in Settings. */
  automatic: boolean;
  /** What Automatic picks for this model. */
  autoConvention: Convention;
  /** The model's name was not recognised, so the automatic pick is a best guess. */
  autoGuess: boolean;
}

// ---- Git (M4) ----------------------------------------------------------------------

export interface GitIdentity {
  name: string;
  email: string;
}

export interface GitInfo {
  backend: "system" | "libgit2";
  notice: string | null;
  tokenHosts: string[];
  identity: GitIdentity;
}

// ---- 0.0.5 shared contracts (WP0). Rust: src-tauri/src/load_settings.rs, dto.rs,
// share/mod.rs, crates/runtime/src/llama/stage.rs. -------------------------------------

/** A llama.cpp backend, as the runtime names it. */
export type BackendKind = "cpu" | "vulkan" | "cuda" | "rocm" | "metal";

/** Why a load failed (crates/runtime/src/llama/failure.rs). */
export type LoadFailure =
  | { code: "out_of_memory" }
  | { code: "invalid_device"; device: string }
  | { code: "missing_runtime_dll" }
  | { code: "port_in_use" }
  | { code: "model_file_invalid" }
  | { code: "context_too_large" }
  | { code: "unknown" };

/** One step of loading a model, or a transition between attempts. */
export type LoadStage =
  | { kind: "starting" }
  | { kind: "readingModel" }
  | { kind: "readingImageReader" }
  | { kind: "loading" }
  | { kind: "restarting"; attempt: number; max: number }
  | { kind: "retryingWithoutSaveMemory"; backend: BackendKind }
  | { kind: "retryingWithLessGpu"; backend: BackendKind; custom?: boolean; chosenLayers?: number }
  | { kind: "steppingDown"; from: BackendKind; to: BackendKind; reason: LoadFailure }
  | { kind: "retryingWithoutFastAttention"; backend: BackendKind };

/** "Use the graphics card": All of it, As much as fits, Don't use it. */
export type GpuUse = "all" | "fit" | "off";
/** "How long it may think"; `noLimit` is shown as "Default". */
export type ThinkingLimit = "noLimit" | "short" | "medium" | "long";

/** One model's "How much it can remember": same as default, Automatic, or a size. */
export type ContextChoice = "default" | "auto" | { fixed: number };
/** A context choice after the defaults: Automatic or a size. */
export type ContextSize = "auto" | { fixed: number };
export type GpuChoice = "default" | GpuUse;
/** An on/off setting of one model (Save memory, Keep the model in memory). */
export type Toggle = "default" | "on" | "off";
/** "Which graphics card": `named` holds the probe's device name (never shown). */
export type DeviceChoice = "default" | "auto" | { named: string };
export type ThinkingChoice = "default" | ThinkingLimit;

/** One model's overrides; every field "default" means "Same as default (…)". */
export interface ModelLoadSettings {
  context: ContextChoice;
  gpu: GpuChoice;
  saveMemory: Toggle;
  keepInMemory: Toggle;
  device: DeviceChoice;
  thinkingLimit: ThinkingChoice;
  /** 0.0.6 item 6: Fine control; absent means every field "auto". */
  fine?: FineControl;
}

/** Why the loaded context is the size it is (the ready line's reason). */
export type ContextSource = "automatic" | "modelSetting" | "defaultSetting" | "trainedLimit";

/** What a load uses after overrides, defaults, projector, trained-length cap and Automatic. */
export interface ResolvedLoad {
  /** Tokens to launch with (Automatic's number when `ctxChoice` is "auto"). */
  ctx: number;
  ctxChoice: ContextSize;
  ctxSource: ContextSource;
  gpu: GpuUse;
  saveMemory: boolean;
  keepInMemory: boolean;
  thinkingLimit: ThinkingLimit;
  backend: string | null;
  device: string | null;
  mmproj: string | null;
  /** 0.0.6 item 6: the Fine control values that reach the command line. */
  fine: FineLaunch;
}

export interface ContextOption {
  choice: ContextSize;
  /** Tokens it means now (Automatic's number for "auto"; 0 when nothing fits). */
  tokens: number;
  fit: Verdict;
}

export interface GpuOption {
  gpu: GpuUse;
  fit: Verdict;
}

/** A device for "Which graphics card"; label it from description and memory, never `name`. */
export interface DeviceOption {
  name: string;
  description: string;
  totalMib: number;
  freeMib: number;
  recommended: boolean;
}

/** Everything the model settings panel shows about one model file (`model_load_info`). */
export interface ModelLoadInfo {
  path: string;
  fileName: string;
  sizeBytes: number;
  trainedCtx: number | null;
  thinking: boolean;
  vision: boolean;
  overrides: ModelLoadSettings;
  defaults: LocalModelSettings;
  resolved: ResolvedLoad;
  fit: Verdict | null;
  /** Not even the shortest memory fits ("Too large for this computer…"). */
  tooLarge: boolean;
  /** Automatic first, then sizes up to the trained length. */
  contextOptions: ContextOption[];
  /** In the order all, fit, off. */
  gpuOptions: GpuOption[];
  devices: DeviceOption[];
  loaded: boolean;
  olderSettingsLoaded: boolean;
  /** Automatic's number for the current load, when loaded with Automatic. */
  autoCtx: number | null;
  /** 0.0.6 item 6: what Fine control may offer for this file on this computer. */
  fineFacts: FineFacts;
  /** 0.0.6 item 6: the instant estimate of where the model goes with these choices. */
  picture: OffloadPreview;
}

/** What is using the local model now (D3). */
export interface ModelBusy {
  chatTurns: number;
  modelChecks: number;
  otherApps: number;
}

/** One model to compare: exactly one of these shapes. */
export type CompareItem = { catalogId: string } | { repo: string; file: string } | { installedPath: string };

export interface LicenseFacts {
  name: string;
  url: string | null;
}

/** From the signed catalog ("From the publisher's model card"); exact matches only. */
export interface CatalogFacts {
  id: string;
  name: string;
  /** Verbatim quote, at most two sentences. */
  summary: string | null;
  /** The publisher's model card URL. */
  source: string | null;
  /** Keys: chat, writing, coding, reasoning, math, tools, images, long-documents, languages, speed. */
  goodAt: string[];
  publisher: string | null;
  parameters: number | null;
  activeParameters: number | null;
  languages: string[];
  released: string | null;
  capabilities: string[];
  license: LicenseFacts | null;
}

/** Shown only under "The uploader says this is made from <base model>. That model's publisher says:". */
export interface BaseModelFacts {
  baseModel: string;
  catalog: CatalogFacts;
}

/** What the Hugging Face uploader lists (sanitised, capped). */
export interface HubFacts {
  repo: string;
  author: string | null;
  pipelineTag: string | null;
  baseModel: string | null;
  languages: string[];
  license: LicenseFacts | null;
  tags: string[];
  downloads: number | null;
  likes: number | null;
  lastModified: string | null;
  gated: boolean;
  cardUrl: string;
}

/** From the GGUF header. */
export interface FileFacts {
  architecture: string | null;
  parameters: number | null;
  contextLength: number | null;
  tools: boolean;
  thinking: boolean;
  vision: boolean;
  moe: boolean;
  quantization: string | null;
  /** Mixture of experts only: experts in the file, and experts used per token. */
  expertCount: number | null;
  expertUsedCount: number | null;
  /** Mixture of experts only: parameters used at a time, when known. */
  activeParameters: number | null;
}

export type YourResultKind = "modelCheck" | "benchmark";

/** One of the user's own results with this file ("Your results"). */
export interface YourResult {
  kind: YourResultKind;
  /** Profile key or benchmark run id. */
  id: string;
  /** Milliseconds since 1970. */
  at: number;
  backend: string | null;
  passed: number;
  total: number;
}

export type CompareProblem = "offline" | "rateLimited" | "gated" | "notFound" | "failed";

/** Everything known about one model to compare (`compare_facts`). */
export interface CompareFacts {
  item: CompareItem;
  title: string;
  file: string | null;
  quant: string | null;
  sizeBytes: number | null;
  installedPath: string | null;
  /** The context the fit was checked at. */
  ctx: number;
  fit: FitResult | null;
  catalog: CatalogFacts | null;
  baseCatalog: BaseModelFacts | null;
  hub: HubFacts | null;
  fileFacts: FileFacts | null;
  results: YourResult[];
  problem: CompareProblem | null;
}

/** One catalog entry for Suggested models (`catalog_list`). */
export interface CatalogListEntry {
  catalog: CatalogFacts;
  repo: string;
  file: string;
  quant: string;
  sizeBytes: number;
  fit: FitResult | null;
  installedPath: string | null;
  results: YourResult[];
}

/** The Other apps page (`share_status`). */
export interface ShareStatus {
  enabled: boolean;
  /** Null until sharing is first switched on. */
  port: number | null;
  listening: boolean;
  error: string | null;
  refused: number;
  inUse: boolean;
  modelCtx: number | null;
}

export type ShareApp = "claudeCode" | "codex" | "claudeApp";
export type SetupSystem = "windows" | "mac" | "any";

export interface SetupStep {
  text: string;
  /** Text for a Copy button. */
  code: string | null;
  /** Full path of the file this step edits. */
  file: string | null;
}

export interface SetupWay {
  title: string;
  system: SetupSystem;
  steps: SetupStep[];
}

/** The setup card for one app (`share_setup`). */
export interface ShareSetup {
  app: ShareApp;
  title: string;
  notTried: boolean;
  ways: SetupWay[];
  notes: string[];
}

/** Coding apps (0.0.5, direction B, experimental): Claude Code and Codex on this computer. */
export type CodingApp = "claudeCode" | "codex";

/** One coding app as the Tools panel shows it (`coding_apps_status`, `find_coding_apps`). */
export interface CodingAppStatus {
  app: CodingApp;
  label: string;
  /** The tool that asks it: `ask_claude_code` or `ask_codex`. */
  tool: string;
  /** The absolute path of the program that runs, as found when the group was turned on. */
  path: string | null;
  version: string | null;
  usable: boolean;
  /** Why it cannot be used, in plain words. */
  problem: string | null;
}

export interface CodingAppsStatus {
  /** The programs were looked for at least once. */
  checked: boolean;
  apps: CodingAppStatus[];
  /** "Claude Code and Codex are not installed on this computer." when neither was found. */
  message: string | null;
}

/** 0.0.6 item 5: Settings → Reverse engineering (`reverse_status`; `src-tauri/src/reverse_cmd.rs`). */
export type ReverseComponent = "rea" | "kuna" | "ghidra";
export type ReverseDecompiler = "kuna" | "ghidra";
export type ReverseDownloadStage = "downloading" | "checking" | "unpacking";
export type ReverseUpdateWhy = { kind: "securityFix"; part: string } | { kind: "notAccepted"; need: string };
export type ReverseComponentState =
  | { kind: "notInstalled" }
  | { kind: "partial"; bytes: number }
  | { kind: "downloading"; done: number; total: number; stage: ReverseDownloadStage }
  | { kind: "installed" }
  | { kind: "broken"; why: "missing" | "blocked" }
  | { kind: "needsUpdate"; why: ReverseUpdateWhy };

export interface ReverseComponentStatus {
  component: ReverseComponent;
  /** Whether this computer can install it; `reason` says why not. */
  available: boolean;
  reason: string | null;
  state: ReverseComponentState;
  installedVersion: string | null;
  pinnedVersion: string;
  /** `rea`, `node`, `kuna`, `ghidra` or `java`, with the installed (else pinned) version. */
  parts: { name: string; version: string }[];
  /** This row's own parts (a decompiler's download adds rea's when rea is missing). */
  downloadBytes: number;
  diskBytes: number;
  update: { part: string; from: string; to: string; downloadBytes: number } | null;
  licenses: { part: string; name: string; url: string }[];
}

export type ReverseRunState =
  | { kind: "stopped" }
  | { kind: "starting" }
  | { kind: "ready" }
  | { kind: "busy"; chatId: number }
  | { kind: "failed"; reason: string };

export interface ReverseStatus {
  /** rea, Kuna and Ghidra, in that order. */
  components: ReverseComponentStatus[];
  /** The decompiler rea uses (installed and chosen). */
  decompiler: ReverseDecompiler | null;
  /** The owner's choice, also when it cannot be used right now. */
  chosen: ReverseDecompiler | null;
  appleTools: "notNeeded" | "present" | "missing";
  running: ReverseRunState;
  /** Kuna's saved analysis, in bytes. */
  cacheBytes: number;
  memoryGb: number | null;
  os: "windows" | "macos" | "linux" | "other";
  partSizes: { component: ReverseComponent; name: string; downloadBytes: number; unpackedBytes: number }[];
}

/** The `reverse-progress` event. */
export interface ReverseProgress {
  component: ReverseComponent;
  part: string;
  /** All parts of the download together while downloading; this step's bytes otherwise. */
  done: number;
  total: number;
  stage: ReverseDownloadStage;
  partDone: number;
  partTotal: number;
  finished: boolean;
  error: ErrorInfo | null;
}

/** 0.0.6 item 5 (WP9h): what the message box does with one file dropped on the window (`reverse_dropped`). */
export type DroppedProgram =
  | { kind: "insert"; path: string; fileName: string }
  | { kind: "addFolder"; fileName: string }
  | { kind: "notReachable"; fileName: string }
  | { kind: "rename"; fileName: string }
  | { kind: "ignore" };

/** The answer to one drop: whether rea is installed, and one entry per path, in order. */
export interface DropAnswer {
  installed: boolean;
  /** Whether the Reverse engineering group is on in this chat (only answered with paths). */
  groupOn: boolean;
  items: DroppedProgram[];
}

// ---- 0.0.6 item 6 (WP10e; Rust: src-tauri/src/load_settings.rs, offload.rs, usage_cmd.rs,
// dto.rs, crates/runtime launch.rs). Usage view and Fine control. -------------------------

/** "Layers on the graphics card": Automatic or a number of llama.cpp's units. */
export type LayersChoice = "auto" | { count: number };
/** "Expert parts on the processor". */
export type ExpertsChoice = "auto" | "all" | { firstLayers: number };
/** "Conversation memory on the graphics card": Automatic (yes) or computer memory. */
export type KvPlace = "auto" | "computerMemory";
export type ThreadsChoice = "auto" | { count: number };
export type FastAttention = "auto" | "on" | "off";
/** "Leave free on the graphics card"; Automatic is 1 GB. */
export type LeaveFree = "auto" | { mib: 512 | 2048 | 4096 };

/** One model's Fine control (every field "auto" means "what the app does without it"). */
export interface FineControl {
  layers: LayersChoice;
  experts: ExpertsChoice;
  kv: KvPlace;
  threads: ThreadsChoice;
  fastAttention: FastAttention;
  leaveFree: LeaveFree;
}

/** Fine control as launch arguments (only what reaches the command line). */
export interface FineLaunch {
  gpuLayers: number | null;
  expertsOnCpu: "none" | "all" | { firstLayers: number };
  kvInComputerMemory: boolean;
  threads: number | null;
  flashAttn: "auto" | "on" | "off";
  fitMarginMib: number | null;
}

/** What Fine control may offer for one model file on this computer. */
export interface FineFacts {
  /** Block count + 1 ("of 41 layers"); null without a readable header. */
  units: number | null;
  /** Block count when the model is a mixture of experts, else null. */
  expertLayers: number | null;
  /** Logical processors of this computer. */
  threads: number;
  /** A graphics device is known for this load. */
  graphics: boolean;
  /** Apple graphics (one memory shared with the processor). */
  apple: boolean;
  /** Apple: the most Apple graphics may use. */
  graphicsLimitBytes: number | null;
}

/** Memory a model needs, in bytes, by part. */
export interface Parts {
  model: number;
  context: number;
  compute: number;
  imageReader: number;
}

/** One memory bar of the picture (bytes). */
export interface MemoryBar {
  name: string;
  totalBytes: number;
  freeBytes: number;
  /** The free figure counts the loaded model as unloaded. */
  freeAfterUnload: boolean;
  otherBytes: number;
  need: Parts;
}

export type PictureSource =
  | { kind: "measured" }
  | {
      kind: "estimate";
      why: "quick" | "noTool" | "failed" | "timedOut" | "notReadable";
      /** Set with "noTool". */
      backend?: BackendKind;
    };

/** The picture's detail of "will it fit?" (placement::judge). */
export type Fits = "fits" | "tight" | "partial" | "wontFitGraphics" | "wontFitAtAll";

/** "Where the model goes" before loading, for one set of choices. */
export interface OffloadPreview {
  /** The choices it answers (the webview drops stale answers). */
  key: string;
  source: PictureSource;
  unified: boolean;
  units: number | null;
  layersOnGraphics: number;
  layersAbout: boolean;
  /** Expert layers whose expert parts run on the processor (the effective count). */
  expertLayersOnProcessor: number | null;
  /** Null on the processor only, and on a Mac. */
  graphics: MemoryBar | null;
  computer: MemoryBar;
  /** Apple: the marker on the single bar. */
  graphicsLimitBytes: number | null;
  mark: Verdict;
  fits: Fits;
  custom: boolean;
}

/** Where the loaded model went (the ready line's memory report). */
export interface Placement {
  graphics: { name: string; use: Parts } | null;
  computer: Parts;
  units: number | null;
  /** Null: only amounts are known. */
  layersOnGraphics: number | null;
  expertLayersOnProcessor: number | null;
  layersAbout: boolean;
  custom: boolean;
}

/** Why a usage figure is not shown. */
export type UsageGap = "noGraphicsCard" | "systemGivesNone" | "notReadHere" | "readFailed";
export type Reading<T> = { kind: "value"; value: T } | { kind: "measuring" } | { kind: "notAvailable"; why: UsageGap };

/** This computer's usage now (`usage_now`), read on this computer only. */
export interface UsageNow {
  graphics: Reading<{
    name: string;
    busyPercent: Reading<number>;
    memoryUsedBytes: Reading<number>;
    memoryTotalBytes: number | null;
  }>;
  processorPercent: Reading<number>;
  /** Logical processors. */
  threads: number;
  memory: Reading<{ usedBytes: number; totalBytes: number }>;
  /** Apple silicon: graphics memory is computer memory. */
  unified: boolean;
}
