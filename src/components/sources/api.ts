// The web search commands (src-tauri/src/search_cmd.rs), as typed calls.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { SearchProvider, SearchSettings, SearchSettingsInput, Source, TestSearch } from "./types";

/** The chat's sources, by number. */
export const listSources = (chatId: number) => invoke<Source[]>("list_sources", { chatId });

export const getSearchSettings = () => invoke<SearchSettings>("get_search_settings");
export const setSearchSettings = (settings: SearchSettingsInput) =>
  invoke<SearchSettings>("set_search_settings", { settings });

/** Stores a key in the system keychain. What comes back says only that it is set. */
export const setSearchKey = (provider: Extract<SearchProvider, "brave" | "tavily">, key: string) =>
  invoke<SearchSettings>("set_search_key", { provider, key });
export const clearSearchKey = (provider: Extract<SearchProvider, "brave" | "tavily">) =>
  invoke<SearchSettings>("clear_search_key", { provider });

/** Runs one search with the saved settings; no page is read. */
export const testSearch = () => invoke<TestSearch>("test_search");

/** A search stored sources for a chat, or more of a source was read. */
export function onSourcesChanged(handler: (chatId: number) => void): Promise<UnlistenFn> {
  return listen<number>("sources-changed", (e) => handler(e.payload));
}
