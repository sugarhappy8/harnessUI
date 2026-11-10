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
  vision: boolean;
  offloadedLayers: number | null;
  totalLayers: number | null;
  error: ErrorInfo | null;
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
}

export interface LocalModelSettings {
  ctx: number;
  backend: string | null;
  device: string | null;
}

export type ChatStreamEvent =
  | { type: "started"; userMessageId: number | null; assistantMessageId: number }
  | { type: "content"; text: string }
  | { type: "reasoning"; text: string }
  | { type: "usage"; promptTokens: number; completionTokens: number }
  | { type: "timings"; predictedPerSecond: number | null; promptPerSecond: number | null }
  | { type: "finished"; status: MessageStatus; finishReason: string | null }
  | { type: "error"; error: ErrorInfo };

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

export interface FitResult {
  verdict: Verdict;
  neededBytes: number;
  availableBytes: number;
  suggestedCtx: number | null;
  partialOffload: boolean;
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
