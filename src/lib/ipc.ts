// Typed wrappers around Tauri commands. The webview has no other way to reach the
// system: no shell, filesystem or HTTP plugin is installed (ARCHITECTURE.md 13.2).
import { Channel, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AgentBrowserProgress,
  ApprovedFolder,
  AuditFilter,
  AuditRow,
  HotkeyStatus,
  BackendInfo,
  CatalogListEntry,
  Chat,
  CompareFacts,
  CompareItem,
  ChatDetail,
  TurnStreamEvent,
  ToolDto,
  McpServerDto,
  McpImportPreview,
  ApprovalRequest,
  SkillList,
  BrowserSettings,
  ComputerStatus,
  GitIdentity,
  GitInfo,
  Convention,
  BrowserStatus,
  UpdateInfo,
  UpdateStatus,
  ChatSummary,
  DiagnosticsReport,
  DiskUsage,
  DocumentInput,
  MentionCheck,
  MentionInput,
  MentionSearch,
  DownloadGroup,
  FirstRunInfo,
  HardwareInfo,
  HubSort,
  ImageInput,
  InstalledModel,
  LocalModelSettings,
  ModelBusy,
  ModelLoadInfo,
  ModelLoadSettings,
  ModelOption,
  PackInfo,
  PackProgress,
  Provider,
  ProviderKind,
  RepoView,
  RuntimeStatus,
  SearchPage,
  ShareApp,
  ShareSetup,
  ShareStatus,
} from "./types";

export interface AppInfo {
  name: string;
  version: string;
  identifier: string;
  os: string;
  arch: string;
  /** The start of the commit the build was made from; empty when the build could not tell. */
  commit: string;
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
/** Picks one of the chat's approved folders as its working folder; null goes back to the first one. */
export const setChatWorkingFolder = (chatId: number, path: string | null) =>
  invoke<void>("set_chat_working_folder", { chatId, path });
export const switchBranch = (chatId: number, messageId: number) =>
  invoke<void>("switch_branch", { chatId, messageId });

export interface SendOptions {
  chatId: number;
  text: string;
  /** The message the new user message answers. Null for the first message. */
  parentId: number | null;
  images?: ImageInput[];
  documents?: DocumentInput[];
  /** Mentions chosen in the picker. */
  mentions?: MentionInput[];
}

export function sendMessage(opts: SendOptions, onEvent: (e: TurnStreamEvent) => void) {
  const channel = new Channel<TurnStreamEvent>();
  channel.onmessage = onEvent;
  return invoke<void>("send_message", { ...opts, onEvent: channel });
}

export function regenerate(
  chatId: number,
  assistantMessageId: number,
  onEvent: (e: TurnStreamEvent) => void,
) {
  const channel = new Channel<TurnStreamEvent>();
  channel.onmessage = onEvent;
  return invoke<void>("regenerate", { chatId, assistantMessageId, onEvent: channel });
}

export const stopGeneration = () => invoke<void>("stop_generation");

// Mentions
export const searchMentions = (chatId: number | null, query: string, limit?: number) =>
  invoke<MentionSearch>("search_mentions", { chatId, query, limit });
export const resolveMentions = (chatId: number, mentions: MentionInput[]) =>
  invoke<MentionCheck[]>("resolve_mentions", { chatId, mentions });

// Models and runtime
export const runtimeStatus = () => invoke<RuntimeStatus>("runtime_status");
export const listBackends = (reprobe: boolean) => invoke<BackendInfo[]>("list_backends", { reprobe });
/**
 * Runs a change of what is loaded. Resolves to `false` when it was replaced or cancelled
 * before it happened (error code `load_replaced`), which is not an error.
 */
async function unlessReplaced(change: Promise<void>): Promise<boolean> {
  try {
    await change;
    return true;
  } catch (err) {
    if (err && typeof err === "object" && (err as { code?: unknown }).code === "load_replaced") return false;
    throw err;
  }
}
/**
 * Loads a local model and resolves when it is ready. Resolves to `false` when another
 * model was picked meanwhile: that load replaced this one, which is not an error.
 * Without `settings` (0.0.5) the model loads with its own saved settings. `whenIdle`
 * (0.0.5, D3): wait until nothing uses the model ("Restart when it ends"; `false` when
 * the wait is cancelled); without it, load now and stop whatever uses the model.
 */
export const loadLocalModel = (path: string, settings?: LocalModelSettings | null, whenIdle?: boolean) =>
  unlessReplaced(
    invoke<void>("load_local_model", { path, settings: settings ?? null, whenIdle: whenIdle ?? null }),
  );
/**
 * Unloads the model. `whenIdle` as for `loadLocalModel`; resolves to `false` when a
 * waiting unload was cancelled or replaced.
 */
export const unloadModel = (whenIdle?: boolean) =>
  unlessReplaced(invoke<void>("unload_model", { whenIdle: whenIdle ?? null }));
export const listModelOptions = () => invoke<ModelOption[]>("list_model_options");
export const getLocalModelSettings = () =>
  invoke<LocalModelSettings>("get_local_model_settings");
export const setLocalModelSettings = (settings: LocalModelSettings) =>
  invoke<void>("set_local_model_settings", { settings });
export const runDiagnostics = (modelPath: string | null) =>
  invoke<DiagnosticsReport>("run_diagnostics", { modelPath });
export const openLogsFolder = () => invoke<void>("open_logs_folder");

// 0.0.5: one model's load settings, reload and what is using the model (WP1b; stubs
// return fixed sample data until it lands).
export const modelLoadInfo = (path: string) => invoke<ModelLoadInfo>("model_load_info", { path });
export const setModelLoadSettings = (path: string, overrides: ModelLoadSettings) =>
  invoke<ModelLoadInfo>("set_model_load_settings", { path, overrides });
/** "Reset to defaults": forgets the model's own settings. */
export const resetModelLoadSettings = (path: string) =>
  invoke<ModelLoadInfo>("reset_model_load_settings", { path });
/**
 * Restarts the loaded model with its current settings (Reload, Apply and restart). `force`
 * restarts even when nothing changed; `whenIdle` waits until nothing uses the model.
 * Resolves to `false` when a waiting reload was cancelled or replaced.
 */
export const reloadLocalModel = (force: boolean, whenIdle: boolean) =>
  unlessReplaced(invoke<void>("reload_local_model", { force, whenIdle }));
/** Cancels the reload, load or unload that waits for the model to be free. */
export const cancelPendingReload = () => invoke<void>("cancel_pending_reload");
export const modelBusy = () => invoke<ModelBusy>("model_busy");

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

// Model manager (M2)

export const hubSearch = (text: string, sort: HubSort, allGguf: boolean) =>
  invoke<SearchPage>("hub_search", { query: { text, sort, allGguf } });
export const hubSearchNext = (next: string) => invoke<SearchPage>("hub_search_next", { next });
export const hubRepo = (repoId: string, ctx: number | null) =>
  invoke<RepoView>("hub_repo", { repoId, ctx });
export const hubTokenStatus = () => invoke<{ saved: boolean; account: string | null }>("hub_token_status");
export const hubSetToken = (token: string) => invoke<string>("hub_set_token", { token });
export const hubClearToken = () => invoke<void>("hub_clear_token");

export const startDownload = (repoId: string, commit: string, label: string, withProjector: boolean) =>
  invoke<string>("download_start", { repoId, commit, label, withProjector });
export const startCatalogDownload = (entryId: string) =>
  invoke<string>("download_catalog_entry", { entryId });
export const pauseDownload = (groupId: string) => invoke<void>("download_pause", { groupId });
export const resumeDownload = (groupId: string) => invoke<void>("download_resume", { groupId });
export const cancelDownload = (groupId: string) => invoke<void>("download_cancel", { groupId });
export const listDownloads = () => invoke<DownloadGroup[]>("list_downloads");
export function onDownloadProgress(handler: (g: DownloadGroup) => void): Promise<UnlistenFn> {
  return listen<DownloadGroup>("download-progress", (e) => handler(e.payload));
}

export const listInstalledModels = () => invoke<InstalledModel[]>("list_installed_models");
export const importModel = (path: string) => invoke<InstalledModel>("import_model", { path });
export const deleteModel = (id: number, deleteFiles: boolean) =>
  invoke<void>("delete_model", { id, deleteFiles });
export const diskUsage = () => invoke<DiskUsage>("disk_usage");
export const setModelsDir = (path: string) => invoke<void>("set_models_dir", { path });
export const hardwareInfo = () => invoke<HardwareInfo>("hardware_info");

// 0.0.5 comparison (WP4a; stubs return fixed sample data until it lands). `ctx` null
// means the defaults' context.
export const catalogList = (ctx: number | null) => invoke<CatalogListEntry[]>("catalog_list", { ctx });
export const compareFacts = (items: CompareItem[], ctx: number | null) =>
  invoke<CompareFacts[]>("compare_facts", { items, ctx });

export const firstRunInfo = () => invoke<FirstRunInfo>("first_run_info");
export const completeFirstRun = () => invoke<void>("complete_first_run");
export const refreshCatalog = () => invoke<number>("refresh_catalog");

// Backend downloads (CUDA, ROCm)
export const listPacks = () => invoke<PackInfo[]>("list_packs");
export const downloadPack = (id: string, acceptLicense: boolean) =>
  invoke<void>("download_pack", { id, acceptLicense });
export const cancelPackDownload = (id: string) => invoke<void>("cancel_pack_download", { id });
export const removePack = (id: string) => invoke<void>("remove_pack", { id });
export function onPackProgress(handler: (p: PackProgress) => void): Promise<UnlistenFn> {
  return listen<PackProgress>("pack-progress", (e) => handler(e.payload));
}
export const getHfEndpoint = () => invoke<string | null>("get_hf_endpoint");
export const setHfEndpoint = (endpoint: string | null) => invoke<void>("set_hf_endpoint", { endpoint });
export const getAttachment = (id: number) => invoke<string>("get_attachment", { id });

// Permissions and audit (M3)
export const listApprovedFolders = (chatId: number | null) =>
  invoke<ApprovedFolder[]>("list_approved_folders", { chatId });
export const approveFolder = (chatId: number | null) => invoke<ApprovedFolder | null>("approve_folder", { chatId });
export const removeApprovedFolder = (id: number) => invoke<void>("remove_approved_folder", { id });
export const listAudit = (filter: AuditFilter) => invoke<AuditRow[]>("list_audit", { filter });
export const exportAudit = (filter: AuditFilter, format: "jsonl" | "csv") =>
  invoke<string | null>("export_audit", { filter, format });
export const stopHotkeyStatus = () => invoke<HotkeyStatus>("stop_hotkey_status");
export const setStopHotkey = (accelerator: string) => invoke<HotkeyStatus>("set_stop_hotkey", { accelerator });
/** Sent when the hotkey is registered at start and whenever it changes. */
export function onStopHotkeyStatus(handler: (s: HotkeyStatus) => void): Promise<UnlistenFn> {
  return listen<HotkeyStatus>("stop-hotkey-status", (e) => handler(e.payload));
}
export function onStopRequested(handler: () => void): Promise<UnlistenFn> {
  return listen("stop-requested", () => handler());
}

// Agent turns: approvals
export const resolveApproval = (id: string, decision: "allowOnce" | "allowForChat" | "deny", note: string | null) =>
  invoke<void>("resolve_approval", { id, decision, note });
// Approval modes: Ask first, or Allow all (only these commands change them)
export const getApprovalMode = (chatId: number) => invoke<boolean>("get_approval_mode", { chatId });
export const setApprovalMode = (chatId: number, allowAll: boolean) =>
  invoke<boolean>("set_approval_mode", { chatId, allowAll });
export const getAllowAllDefault = () => invoke<boolean>("get_allow_all_default");
export const setAllowAllDefault = (on: boolean) => invoke<void>("set_allow_all_default", { on });

// Tools and MCP servers
export const listChatTools = (chatId: number) => invoke<ToolDto[]>("list_chat_tools", { chatId });
export const setChatTools = (chatId: number, enabled: string[]) =>
  invoke<void>("set_chat_tools", { chatId, enabled }).then(() => {
    // [0.0.4 context] The Context tab reads the next request again.
    window.dispatchEvent(new CustomEvent("chat-tools-changed", { detail: chatId }));
  });
// Adaptive tool selection: which of the tools that are on a turn shows the model.
export const getToolSelection = () => invoke<import("./types").ToolSelection>("get_tool_selection");
export const setToolSelection = (auto: boolean) =>
  invoke<import("./types").ToolSelection>("set_tool_selection", { auto });
export const clearChatGrants = (chatId: number) => invoke<void>("clear_chat_grants", { chatId });
export const listMcpServers = () => invoke<McpServerDto[]>("list_mcp_servers");
export const previewMcpConfig = (json: string) => invoke<McpImportPreview[]>("preview_mcp_config", { json });
export const addMcpServers = (json: string, names: string[], moveCredentials: boolean) =>
  invoke<void>("add_mcp_servers", { json, names, moveCredentials });
export const removeMcpServer = (name: string) => invoke<void>("remove_mcp_server", { name });
export const setMcpServerEnabled = (name: string, enabled: boolean) =>
  invoke<void>("set_mcp_server_enabled", { name, enabled });
export const restartMcpServer = (name: string) => invoke<void>("restart_mcp_server", { name });
export const setMcpToolPref = (
  server: string,
  tool: string,
  enabled: boolean,
  allowAnyArgs: boolean,
  alwaysAsk: boolean,
) => invoke<void>("set_mcp_tool_pref", { server, tool, enabled, allowAnyArgs, alwaysAsk });
export const mcpServerLog = (name: string) => invoke<string[]>("mcp_server_log", { name });

export function onApprovalRequest(handler: (r: ApprovalRequest) => void): Promise<UnlistenFn> {
  return listen<ApprovalRequest>("approval-request", (e) => handler(e.payload));
}

export function onApprovalClosed(handler: (id: string) => void): Promise<UnlistenFn> {
  return listen<string>("approval-closed", (e) => handler(e.payload));
}

/** A chat's approval mode changed in the app (null: every chat's may have). */
export function onApprovalModeChanged(handler: (chatId: number | null) => void): Promise<UnlistenFn> {
  return listen<number | null>("approval-mode-changed", (e) => handler(e.payload));
}

export function onMcpChanged(handler: () => void): Promise<UnlistenFn> {
  return listen("mcp-changed", () => handler());
}
export const listServerTools = (server: string) => invoke<ToolDto[]>("list_server_tools", { server });

// Skills
export const listSkills = () => invoke<SkillList>("list_skills");
export const importSkill = (zip: boolean) => invoke<string | null>("import_skill", { zip });
export const setSkillEnabled = (name: string, enabled: boolean) =>
  invoke<void>("set_skill_enabled", { name, enabled });
export const deleteSkill = (name: string) => invoke<void>("delete_skill", { name });
export const getSkillSource = (name: string) => invoke<string>("get_skill_source", { name });
export const validateSkill = (text: string, folder: string) => invoke<string[]>("validate_skill", { text, folder });
export const saveSkill = (folder: string, text: string, isNew: boolean) =>
  invoke<string[]>("save_skill", { folder, text, isNew });
export const setSharedSkills = (enabled: boolean) => invoke<void>("set_shared_skills", { enabled });
export const openSkillsFolder = () => invoke<void>("open_skills_folder");
export const loadedSkills = (chatId: number) => invoke<string[]>("loaded_skills", { chatId });
export const unloadSkill = (chatId: number, name: string) => invoke<void>("unload_skill", { chatId, name });
export const setChatThinking = (chatId: number, on: boolean) => invoke<void>("set_chat_thinking", { chatId, on });

// 0.0.5 Other apps use the loaded model (WP5a; stubs return fixed sample data and change
// nothing until it lands).
export const shareStatus = () => invoke<ShareStatus>("share_status");
export const setShareEnabled = (on: boolean) => invoke<ShareStatus>("set_share_enabled", { on });
export const setSharePort = (port: number) => invoke<ShareStatus>("set_share_port", { port });
/** The sharing key, shown only on request. */
export const revealShareKey = () => invoke<string>("reveal_share_key");
export const newShareKey = () => invoke<ShareStatus>("new_share_key");
export const shareSetup = (app: ShareApp) => invoke<ShareSetup>("share_setup", { app });

// Updates
export const updateStatus = () => invoke<UpdateStatus>("update_status");
export const setUpdateAutoCheck = (on: boolean) => invoke<void>("set_update_auto_check", { on });
export const checkForUpdate = () => invoke<UpdateInfo | null>("check_for_update");
export const installUpdate = () => invoke<void>("install_update");
/** Sent by the automatic check at start when a newer version exists. */
export function onUpdateAvailable(handler: (u: UpdateInfo) => void): Promise<UnlistenFn> {
  return listen<UpdateInfo>("update-available", (e) => handler(e.payload));
}

// Terminal
export const getTerminalPassThrough = () => invoke<string[]>("get_terminal_pass_through");
export const setTerminalPassThrough = (names: string[]) =>
  invoke<string[]>("set_terminal_pass_through", { names });

// Browser tool
export const browserStatus = () => invoke<BrowserStatus>("browser_status");
export const setBrowserSettings = (settings: BrowserSettings) => invoke<void>("set_browser_settings", { settings });
export const chooseBrowserPath = (clear: boolean) => invoke<string | null>("choose_browser_path", { clear });
export const downloadAgentBrowser = () => invoke<void>("download_agent_browser");
export const cancelAgentBrowserDownload = () => invoke<void>("cancel_agent_browser_download");
export const removeAgentBrowser = () => invoke<void>("remove_agent_browser");
export function onAgentBrowserProgress(handler: (p: AgentBrowserProgress) => void): Promise<UnlistenFn> {
  return listen<AgentBrowserProgress>("agent-browser-progress", (e) => handler(e.payload));
}

// Computer use
export const computerStatus = (modelRef: string | null) => invoke<ComputerStatus>("computer_status", { modelRef });
/** Saves the convention chosen for a model; `null` goes back to Automatic. */
export const setComputerProfile = (modelRef: string, convention: Convention | null) =>
  invoke<void>("set_computer_profile", { modelRef, convention });

// Git
export const gitInfo = () => invoke<GitInfo>("git_info");
export const setGitToken = (host: string, token: string) => invoke<string[]>("set_git_token", { host, token });
export const removeGitToken = (host: string) => invoke<string[]>("remove_git_token", { host });
export const setGitIdentity = (identity: GitIdentity) => invoke<void>("set_git_identity", { identity });
export const minimizeMainWindow = () => invoke<void>("minimize_main_window");

// Saved tool output (0.0.4): the whole text of a result the model saw shortened.
export interface SavedToolOutput {
  id: number;
  tool: string;
  lines: number;
  bytes: number;
  text: string;
}
export const getToolOutput = (chatId: number, id: number) =>
  invoke<SavedToolOutput>("get_tool_output", { chatId, id });
/** The output the app saved for one tool call of the chat, if any. */
export const toolOutputForCall = (chatId: number, callId: string) =>
  invoke<number | null>("tool_output_for_call", { chatId, callId });

// Tested settings per model, and coding benchmarks (0.0.4, src-tauri/src/evals/).
export const getModelProfile = (modelRef: string) =>
  invoke<import("../components/evals/types").ProfileView>("get_model_profile", { modelRef });
export const forgetModelProfile = (key: string) => invoke<boolean>("forget_model_profile", { key });
export const startCapabilityCheck = (modelRef: string) =>
  invoke<import("../components/evals/types").ModelProfile>("start_capability_check", { modelRef });
export const cancelEval = () => invoke<boolean>("cancel_eval");
export const listBenchTasks = () =>
  invoke<import("../components/evals/types").BenchTaskInfo[]>("list_bench_tasks");
export const startBenchmark = (modelRef: string) =>
  invoke<import("../components/evals/types").BenchRun>("start_benchmark", { modelRef });
export const listBenchRuns = () =>
  invoke<import("../components/evals/types").BenchSummary[]>("list_bench_runs");
export const getBenchRun = (id: string) =>
  invoke<import("../components/evals/types").BenchRun>("get_bench_run", { id });
export function onEvalProgress(
  handler: (p: import("../components/evals/types").EvalProgress) => void,
): Promise<UnlistenFn> {
  return listen<import("../components/evals/types").EvalProgress>("eval-progress", (e) => handler(e.payload));
}
