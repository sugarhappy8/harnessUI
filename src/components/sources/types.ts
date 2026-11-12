// What the web search feature sends to the window (src-tauri/src/search_cmd.rs).

/** One source a web search stored for a chat. Everything in it is text from the web:
 *  show it as plain text only. */
export interface Source {
  /** The number an answer cites it by: `[n]`. */
  n: number;
  url: string;
  title: string;
  /** The host, without `www.`. */
  site: string;
  /** The search service's summary, when that is what the model was given. */
  snippet: string;
  /** The text of the page the model was given. Empty when the page was not read. */
  excerpt: string;
  /** SHA-256 of the page's extracted text at the last read. */
  contentSha256: string | null;
  official: boolean;
  /** "matches tokio 1.38", "for tokio 0.2". */
  versionNote: string | null;
  /** The searches that found it, oldest first. */
  queries: string[];
  /** When the page was last read (Unix ms); null when it was not read. */
  fetchedAt: number | null;
  createdAt: number;
}

export type SearchProvider = "duckduckgo" | "brave" | "tavily" | "searxng";

/** The search settings. A key is never sent to the window: only whether one is set. */
export interface SearchSettings {
  provider: SearchProvider;
  searxngUrl: string;
  /** How many of the top results are read (0: none). */
  pages: number;
  versionMatching: boolean;
  braveKeySet: boolean;
  tavilyKeySet: boolean;
}

/** What can be saved: the settings without the key flags. */
export type SearchSettingsInput = Pick<SearchSettings, "provider" | "searxngUrl" | "pages" | "versionMatching">;

export interface TestSearch {
  /** The service's name. */
  provider: string;
  results: { title: string; site: string }[];
}
