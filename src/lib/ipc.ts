// Typed wrappers around Tauri commands. The webview has no other way to reach the
// system: no shell, filesystem or HTTP plugin is installed (ARCHITECTURE.md 13.2).
import { Channel, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  BackendInfo,
  Chat,
  ChatDetail,
  ChatStreamEvent,
  ChatSummary,
  DiagnosticsReport,
  LocalModelSettings,
  ModelOption,
  Provider,
  ProviderKind,
  RuntimeStatus,
} from "./types";

export interface AppInfo {
  name: string;
  version: string;
  identifier: string;
  os: string;
  arch: string;
}

export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export const appInfo = () => invoke<AppInfo>("app_info");

// Chats
export const listChats = (query: string | null) => invoke<ChatSummary[]>("list_chats", { query });
export const createChat = (modelRef: string | null) => invoke<Chat>("create_chat", { modelRef });
export const getChat = (chatId: number) => invoke<ChatDetail>("get_chat", { chatId });
export const renameChat = (chatId: number, title: string) =>
  invoke<void>("rename_chat", { chatId, title });
export const deleteChat = (chatId: number) => invoke<void>("delete_chat", { chatId });
export const setChatModel = (chatId: number, modelRef: string | null) =>
  invoke<void>("set_chat_model", { chatId, modelRef });
export const setSystemPrompt = (chatId: number, systemPrompt: string | null) =>
  invoke<void>("set_system_prompt", { chatId, systemPrompt });
export const switchBranch = (chatId: number, messageId: number) =>
  invoke<void>("switch_branch", { chatId, messageId });

export interface SendOptions {
  chatId: number;
  text: string;
  /** The message the new user message answers. Null for the first message. */
  parentId: number | null;
}

export function sendMessage(opts: SendOptions, onEvent: (e: ChatStreamEvent) => void) {
  const channel = new Channel<ChatStreamEvent>();
  channel.onmessage = onEvent;
  return invoke<void>("send_message", { ...opts, onEvent: channel });
}

export function regenerate(
  chatId: number,
  assistantMessageId: number,
  onEvent: (e: ChatStreamEvent) => void,
) {
  const channel = new Channel<ChatStreamEvent>();
  channel.onmessage = onEvent;
  return invoke<void>("regenerate", { chatId, assistantMessageId, onEvent: channel });
}

export const stopGeneration = () => invoke<void>("stop_generation");

// Models and runtime
export const runtimeStatus = () => invoke<RuntimeStatus>("runtime_status");
export const listBackends = (reprobe: boolean) => invoke<BackendInfo[]>("list_backends", { reprobe });
export const loadLocalModel = (path: string, settings: LocalModelSettings) =>
  invoke<void>("load_local_model", { path, settings });
export const unloadModel = () => invoke<void>("unload_model");
export const listModelOptions = () => invoke<ModelOption[]>("list_model_options");
export const getLocalModelSettings = () =>
  invoke<LocalModelSettings>("get_local_model_settings");
export const setLocalModelSettings = (settings: LocalModelSettings) =>
  invoke<void>("set_local_model_settings", { settings });
export const runDiagnostics = (modelPath: string | null) =>
  invoke<DiagnosticsReport>("run_diagnostics", { modelPath });

export function onRuntimeStatus(handler: (s: RuntimeStatus) => void): Promise<UnlistenFn> {
  return listen<RuntimeStatus>("runtime-status", (e) => handler(e.payload));
}

// Providers (OpenAI-compatible endpoints)
export const listProviders = () => invoke<Provider[]>("list_providers");
export const saveProvider = (p: {
  id: number | null;
  name: string;
  kind: ProviderKind;
  baseUrl: string;
  apiKey: string | null;
  clearApiKey: boolean;
}) => invoke<Provider>("save_provider", { provider: p });
export const deleteProvider = (id: number) => invoke<void>("delete_provider", { id });
export const testProvider = (id: number) => invoke<ModelOption[]>("test_provider", { id });

// Settings
export const getSetting = <T>(key: string) => invoke<T | null>("get_setting", { key });
export const setSetting = (key: string, value: unknown) => invoke<void>("set_setting", { key, value });
