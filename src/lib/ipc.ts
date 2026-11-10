// Typed wrappers around Tauri commands. The webview has no other way to reach
// the system: no shell, filesystem or HTTP plugin is installed.
import { invoke } from "@tauri-apps/api/core";

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

export async function appInfo(): Promise<AppInfo> {
  return invoke<AppInfo>("app_info");
}
