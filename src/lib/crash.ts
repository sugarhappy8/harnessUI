// What the app keeps of an error that stopped part of a screen from being drawn (0.2.1). The
// error boundaries show it under *Details* and write it to the developer console: the error's
// name and the start of its message, with anything that could be private left out (a file
// path, which can hold the home folder's name; a web address, which can hold a key; anything
// shaped like a key). Nothing is sent anywhere: the webview has no log of its own on disk.

/** At most this many characters of a message are kept. */
export const MESSAGE_LIMIT = 200;

const HIDDEN = "…";

/** Patterns of the private parts a message can carry, each replaced by an ellipsis (`$1` keeps
 *  what a pattern matched before the part). No lookbehind: Safari before 16.4 cannot read it. */
const PRIVATE: [RegExp, string][] = [
  // Web and app addresses (https://…, ipc://…, asset://…), whose query can hold a key.
  [/\b[a-z][a-z0-9+.-]*:\/\/\S+/gi, HIDDEN],
  // Windows paths: C:\Users\name\…, C:/Users/…, \\server\share\….
  [/\b[a-z]:[\\/][^\s'"`<>|]*/gi, HIDDEN],
  [/\\\\[^\s'"`<>|]+/g, HIDDEN],
  // Home-relative and absolute POSIX paths with at least one folder: ~/…, /home/name/….
  [/~[\\/][^\s'"`<>|]*/g, HIDDEN],
  [/(^|[\s'"`(=:,])\/(?:[^\s/'"`<>|]+\/)+[^\s'"`<>|]*/g, `$1${HIDDEN}`],
  // Key-shaped words: sk-…, hf_…, ghp_…, xoxb-…, and long runs of letters and digits.
  [/\b(?:sk|pk|rk|hf|ghp|gho|ghs|github_pat|xox[a-z])[-_][\w-]{6,}/gi, HIDDEN],
  [/\b(?=[\w-]*\d)[\w-]{32,}\b/g, HIDDEN],
];

/** `text` with its private parts left out and cut to MESSAGE_LIMIT characters. */
export function cleanMessage(text: string): string {
  let out = text;
  for (const [pattern, by] of PRIVATE) out = out.replace(pattern, by);
  out = out.replace(/\s+/g, " ").trim();
  return out.length > MESSAGE_LIMIT ? `${out.slice(0, MESSAGE_LIMIT - 1)}${HIDDEN}` : out;
}

/** The thrown value as an Error (a string or anything else thrown becomes one). */
export function asError(thrown: unknown): Error {
  if (thrown instanceof Error) return thrown;
  return new Error(typeof thrown === "string" ? thrown : String(thrown));
}

/** What is kept of `thrown`: its name and its cleaned message. */
export function crashSummary(thrown: unknown): { name: string; message: string } {
  const error = asError(thrown);
  const name = /^[A-Za-z][\w$]{0,40}$/.test(error.name) ? error.name : "Error";
  return { name, message: cleanMessage(error.message) };
}

/** Writes what is kept of `thrown` to the developer console, with the part of the app that
 *  could not be drawn (a component's name). Never the stack or the component tree, which name
 *  the app's files. */
export function reportCrash(where: string, thrown: unknown): void {
  const { name, message } = crashSummary(thrown);
  console.error(`[crash] ${where} could not be drawn: ${name}: ${message}`);
}
