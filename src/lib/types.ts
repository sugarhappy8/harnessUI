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
