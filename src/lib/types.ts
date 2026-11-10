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
